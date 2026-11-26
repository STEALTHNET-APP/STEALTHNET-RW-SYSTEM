use crate::{AppState, ServerConfig, router};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use sn_domain::access::AdminRole;
use sn_domain::client::{RegistrationSource, TelegramProfile};
use sn_domain::ids::{ClientId, SquadId};
use sn_domain::money::{Currency, Stars};
use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
use sn_domain::payment::{NewPayment, ProviderId, providers};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::settings::keys;
use sn_domain::tariff::{CurrencyPrices, DevicePolicy, Tariff};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::ModuleRegistry;
use sn_payments::ProviderRegistry;
use sn_remnawave::fake::FakeRemnawave;
use sn_remnawave::{InternalSquad, Node, SquadInfo};
use sn_services::{EnvFallbacks, FixedClock, ServiceDeps, Services, SystemClock};
use sn_store::testing::{TestDb, test_store};
use sn_store::{Db, NewAdmin, NewCategory, NewPeriod, NewTariff, SecretBox, Store, StoreOptions};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

mod admins;
mod alerts;
mod analytics;
mod antilopay;
mod api_keys;
mod app_links;
mod audit_journal;
mod backups;
mod balance_off;
mod bot_content;
mod broadcast;
mod cabinet;
mod captcha;
mod channels;
mod client_access;
mod client_email;
mod client_errors;
mod client_payments;
mod config_warnings;
mod connect_apps;
mod cryptomus;
mod documents;
mod email_signup;
mod ext_modules;
mod freekassa;
mod gram;
mod heleket;
mod hosts;
mod import;
mod landing;
mod languages;
mod legacy_webhooks;
mod mail;
mod mail_errors;
mod maintenance;
mod migrated_providers;
mod mini_account;
mod mini_support;
mod miniapp;
mod module_packages;
mod multi_subscriptions;
mod oauth;
mod password_reset;
mod payment_journal;
mod photos;
mod promo_days;
mod provider_check;
mod proxy;
mod remna_link;
mod robokassa;
mod robots;
mod rollypay;
mod security_headers;
mod server_clock;
mod site_limits;
mod support_files;
mod support_list;
mod support_mode;
mod support_role;
mod telegram_link;
mod telegram_webhook;
mod tribute;
mod utm;
mod wata;

const SECRET: &str = "0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "очень-надёжный-пароль";

fn services(store: Store) -> Services {
    Services::new(ServiceDeps {
        store,
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: None,
        clock: Arc::new(SystemClock),
        env: EnvFallbacks::default(),
    })
}

fn offline_state() -> AppState {
    let store = Store::connect_lazy(
        "postgres://sn:sn@127.0.0.1:1/sn_test",
        StoreOptions {
            acquire_timeout: std::time::Duration::from_millis(300),
            ..StoreOptions::default()
        },
    )
    .unwrap();
    AppState::new(ServerConfig::default(), services(store))
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

struct Http {
    db: TestDb,
    state: AppState,
    clock: Arc<FixedClock>,
    remna: Arc<FakeRemnawave>,
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
}

async fn fetch_file(
    h: &Http,
    path: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    if let Some(etag) = if_none_match {
        builder = builder.header(header::IF_NONE_MATCH, etag);
    }
    let response = router(h.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn http() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::catalog_only()),
        secrets: Some(SecretBox::from_base64(&SecretBox::generate_key_base64()).unwrap()),
        clock: clock.clone(),
        env: EnvFallbacks::default(),
    });
    services.reload().await.unwrap();
    let remna = Arc::new(FakeRemnawave::new());
    services.set_remna(Some(remna.clone()));
    Some(Http {
        db,
        state: AppState::new(ServerConfig::default(), services),
        clock,
        remna,
    })
}

impl Http {
    fn services(&self) -> &Services {
        &self.state.services
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        (status, headers, body_json(response).await)
    }

    async fn get(&self, path: &str, token: &str) -> (StatusCode, Value) {
        let (status, _, body) = self.send(Method::GET, path, Some(token), None).await;
        (status, body)
    }

    async fn write(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let (status, _, body) = self.send(method, path, Some(token), Some(body)).await;
        (status, body)
    }

    async fn login(&self, email: &str) -> String {
        let (status, _, body) = self
            .send(
                Method::POST,
                "/api/admin/auth/login",
                None,
                Some(json!({ "email": email, "password": PASSWORD })),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    async fn owner(&self) -> String {
        self.services()
            .auth()
            .upsert_owner("owner@example.com", PASSWORD)
            .await
            .unwrap();
        self.login("owner@example.com").await
    }

    async fn admin(&self, email: &str, role: AdminRole) -> String {
        let hash = sn_services::auth::hash_password(PASSWORD).unwrap();
        {
            let mut conn = self.services().store().conn().await.unwrap();
            conn.admins()
                .create(&NewAdmin {
                    email: email.into(),
                    password_hash: hash,
                    role,
                    must_change_password: false,
                })
                .await
                .unwrap();
        }
        self.login(email).await
    }

    async fn client(&self, telegram_id: i64) -> ClientId {
        self.services()
            .clients()
            .ensure_telegram(
                &TelegramProfile {
                    id: telegram_id,
                    username: Some("morozov_a".into()),
                    first_name: Some("Алексей".into()),
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

    async fn client_token(&self, client: ClientId) -> String {
        self.services()
            .auth()
            .client_session_for(
                client,
                sn_store::SessionChannel::Cabinet,
                &Default::default(),
                false,
            )
            .await
            .unwrap()
            .token
    }

    async fn set_bot(&self) {
        let mut conn = self.services().store().conn().await.unwrap();
        conn.settings()
            .set_as(keys::BOT_USERNAME, &"stealth_vpn_bot", None)
            .await
            .unwrap();
    }

    async fn tariff(&self) -> Tariff {
        let mut tx = self.services().store().begin().await.unwrap();
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
                squads: vec![SquadId(Uuid::from_u128(1))],
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
    }

    async fn pending_payment(
        &self,
        client_id: ClientId,
        tariff: &Tariff,
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
        let mut tx = self.services().store().begin().await.unwrap();
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
                provider: ProviderId::from(providers::CRYPTOPAY),
                method: None,
                amount: q.total.into(),
                expires_at: None,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        payment
    }
}

fn totp_code(secret: &str, at: DateTime<Utc>) -> String {
    let key = sn_services::totp::base32_decode(secret).unwrap();
    sn_services::totp::code_at(
        &key,
        (at.timestamp() / sn_services::totp::STEP_SECONDS) as u64,
        sn_services::totp::DIGITS,
    )
    .unwrap()
}

#[tokio::test]
async fn версия_отвечает_без_базы() {
    let response = router(offline_state())
        .oneshot(Request::get("/api/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["remnawave"], "3.4.11");
}

#[tokio::test]
async fn каждый_ответ_несёт_идентификатор_запроса() {
    let response = router(offline_state())
        .oneshot(Request::get("/api/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let generated = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert_eq!(generated.len(), 36);
    let response = router(offline_state())
        .oneshot(
            Request::get("/api/version")
                .header("x-request-id", "req-42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["x-request-id"], "req-42");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn здоровье_честно_говорит_что_базы_нет() {
    let response = router(offline_state())
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn вебхук_remnawave_без_секрета_и_с_чужой_подписью() {
    let state = offline_state();
    let body = r#"{"scope":"service","event":"service.panel_started","timestamp":"2026-01-07T11:57:29.426Z","data":{}}"#;
    let request = |signature: Option<&str>| {
        let mut builder = Request::post("/api/remnawave/webhook")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(signature) = signature {
            builder = builder.header("x-remnawave-signature", signature);
        }
        builder.body(Body::from(body)).unwrap()
    };
    let response = router(state.clone()).oneshot(request(None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    state.services.set_remna_webhook_secret(Some(SECRET.into()));
    let response = router(state.clone())
        .oneshot(request(Some("00ff")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "remnawave.bad_signature"
    );
}

#[tokio::test]
async fn старый_адрес_вебхука_telegram_не_принимает_обновления() {
    let response = router(offline_state())
        .oneshot(
            Request::post("/api/telegram/webhook")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "telegram.webhook_disabled"
    );
}

#[tokio::test]
async fn админка_без_входа_закрыта() {
    let state = offline_state();
    for path in [
        "/api/admin/me",
        "/api/admin/modules",
        "/api/admin/dashboard",
        "/api/admin/payments",
        "/api/admin/settings/core",
        "/api/admin/m/gifts/anything",
    ] {
        let response = router(state.clone())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let response = router(state)
        .oneshot(Request::get("/api/nothing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"]["code"], "not_found");
}

#[tokio::test]
async fn вход_администратора_и_переключение_модуля() {
    let Some(db) = sn_store::testing::test_store().await else {
        return;
    };
    let services = services(db.store.clone());
    services.reload().await.unwrap();
    services
        .auth()
        .upsert_owner("owner@example.com", PASSWORD)
        .await
        .unwrap();
    let state = AppState::new(ServerConfig::default(), services);
    let login = router(state.clone())
        .oneshot(
            Request::post("/api/admin/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"owner@example.com","password":"очень-надёжный-пароль"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    assert!(login.headers().get(header::SET_COOKIE).is_some());
    let token = body_json(login).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let toggle = router(state.clone())
        .oneshot(
            Request::patch("/api/admin/modules/gifts")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(toggle.status(), StatusCode::OK);
    assert_eq!(body_json(toggle).await["active"], true);
    let removed = router(state.clone())
        .oneshot(
            Request::patch("/api/admin/modules/nalog")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(removed.status().is_client_error(), "{}", removed.status());
    let menu = router(state)
        .oneshot(
            Request::get("/api/admin/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let labels: Vec<String> = body_json(menu).await["menu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        labels,
        ["Рассылки", "Промокоды", "Рефералы", "Подарки", "UTM-метки"]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn вход_по_коду_totp_и_смена_пароля_через_http() {
    let Some(h) = http().await else { return };
    let token = h.owner().await;
    let last_method = |admins: &Value| admins[0]["last_login_method"].clone();
    let (_, admins) = h.get("/api/admin/admins", &token).await;
    assert_eq!(last_method(&admins), "password");
    let (status, setup) = h
        .write(Method::POST, "/api/admin/auth/2fa/setup", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_string();
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/auth/2fa/enable",
            &token,
            json!({ "secret": secret, "code": totp_code(&secret, start()), "password": PASSWORD }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/admin/auth/login",
            None,
            Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "auth.totp_required");
    h.clock.set(start() + Duration::seconds(30));
    let (status, headers, body) = h
        .send(
            Method::POST,
            "/api/admin/auth/login",
            None,
            Some(json!({
                "email": "owner@example.com",
                "password": PASSWORD,
                "totp": totp_code(&secret, start() + Duration::seconds(30)),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, admins) = h
        .get("/api/admin/admins", body["token"].as_str().unwrap())
        .await;
    assert_eq!(last_method(&admins), "totp");
    let cookie = headers[header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(cookie.starts_with("sn_admin="));
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let second = body["token"].as_str().unwrap().to_string();
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/auth/password",
            &second,
            json!({ "current": PASSWORD, "new": "новый-надёжный-пароль" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let renewed = body["token"].as_str().unwrap().to_string();
    for old in [&token, &second] {
        assert_eq!(
            h.get("/api/admin/me", old).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, me) = h.get("/api/admin/me", &renewed).await;
    assert_eq!(status, StatusCode::OK);
    assert!(me["permissions"].as_array().unwrap().len() >= 11);
    let (_, admins) = h.get("/api/admin/admins", &renewed).await;
    assert_eq!(last_method(&admins), "totp");
    let (status, security) = h.get("/api/admin/auth/security", &renewed).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(security["totp_enabled"], true);
    let (status, _, _) = h
        .send(Method::POST, "/api/admin/auth/logout", Some(&renewed), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.get("/api/admin/me", &renewed).await.0,
        StatusCode::UNAUTHORIZED
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn поддержка_не_видит_деньги_а_наблюдатель_не_пишет() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let readonly = h.admin("readonly@example.com", AdminRole::ReadOnly).await;
    let client = h.client(5_093_118_422).await;
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/balance", client.0),
            &owner,
            json!({ "delta": { "minor": 420, "currency": "USD" }, "comment": "подарок", "idempotency_key": "r1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let card = format!("/api/admin/clients/{}", client.0);
    let (status, body) = h.get(&card, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["balance"]["minor"], 420);
    assert_eq!(body["stats"]["paid_total"]["minor"], 0);
    assert_eq!(body["stats"]["payments_count"], 0);
    assert_eq!(body["bot_available"], false);
    let (status, body) = h.get(&card, &support).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("balance").is_none() && body.get("referral_balance").is_none());
    assert!(body.get("balance_held").is_none(), "{body}");
    assert!(body["stats"].get("paid_total").is_none());
    assert!(body["stats"].get("payments_count").is_none());
    assert_eq!(body["stats"]["referrals_count"], 0);
    let (status, history) = h.get(&format!("{card}/history"), &support).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        history["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] != "balance_changed")
    );
    assert_eq!(history["total"], 1);
    let (status, history) = h.get(&format!("{card}/history"), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        history["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "event" && e["kind"] == "balance_changed")
    );
    assert_eq!(history["total"], 2);
    assert_eq!(history["next"], Value::Null);
    for path in [
        "/api/admin/payments",
        "/api/admin/settings/core",
        "/api/admin/tariffs",
    ] {
        assert_eq!(
            h.get(path, &support).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let (status, _) = h
        .write(
            Method::POST,
            &format!("{card}/balance"),
            &support,
            json!({ "delta": { "minor": 100, "currency": "USD" }, "comment": "x", "idempotency_key": "r2" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, dashboard) = h.get("/api/admin/dashboard", &support).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dashboard["revenue"].is_null());
    assert!(
        !h.get("/api/admin/support/tickets", &support)
            .await
            .0
            .is_client_error()
    );
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("{card}/note"),
            &support,
            json!({ "note": "звонил" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(h.get(&card, &readonly).await.0, StatusCode::OK);
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("{card}/note"),
            &readonly,
            json!({ "note": "x" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .write(Method::POST, &format!("{card}/block"), &readonly, json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for path in [
        "/api/admin/infra/nodes",
        "/api/admin/support/tickets",
        "/api/admin/modules",
        "/api/admin/settings/remnawave",
    ] {
        assert_eq!(
            h.get(path, &readonly).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let (status, me) = h.get("/api/admin/me", &readonly).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["permissions"], json!(["clients_read"]));
    h.db.cleanup().await;
}

#[tokio::test]
async fn настройки_ядра_оплаты_и_remnawave_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "currency": "RUB" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["settings"]["currency"], "RUB");
    let (_, body) = h.get("/api/admin/settings/payments", &owner).await;
    assert_eq!(body["currency"], "RUB");
    assert_eq!(body["order"][0], "platega");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/payments",
            &owner,
            json!({ "order": ["yoomoney", "platega", "balance"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["order"], json!(["yoomoney", "platega"]));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/payments",
            &owner,
            json!({ "order": ["paybox"] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "settings.invalid_field");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({ "enabled": true, "days": 3, "squads": [], "devices": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "settings.invalid_field");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({ "enabled": true, "days": 3, "squads": [Uuid::from_u128(1)], "devices": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(h.services().settings().trial.variants[0].days, 3);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/yoomoney",
            &owner,
            json!({ "config": { "unknown_field": "x" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/payments/providers/cryptopay",
            &owner,
            json!({ "enabled": true, "config": { "api_token": "4321:SECRET" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"]["settings"]["api_token"]["filled"], true);
    assert!(!body.to_string().contains("4321:SECRET"));
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/remnawave/webhook-secret",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let secret = body["secret"].as_str().unwrap().to_string();
    assert!(body["env"].as_str().unwrap().contains(&secret));
    let (status, body) = h.get("/api/admin/settings/remnawave", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["webhook_secret_set"], true);
    assert!(!body.to_string().contains(&secret));
    h.services().set_remna(Some(h.remna.clone()));
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/remnawave/check",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], "3.4.11");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/bot",
            &owner,
            json!({ "username": "@stealth_vpn_bot" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["username"], "stealth_vpn_bot");
    assert_eq!(body["status"], json!({ "state": "off" }));
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/bot/restart",
            &owner,
            json!({ "take_over": true }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "bot.takeover_not_needed");
    h.services()
        .bot_runtime()
        .set(sn_services::BotState::Failed {
            problem: sn_services::BotProblem::Webhook,
            detail: Some("old.example.com".into()),
            at: Utc::now(),
            retry_at: None,
        });
    let support = h.admin("bot-support@example.com", AdminRole::Support).await;
    let (status, _) = h
        .write(
            Method::POST,
            "/api/admin/settings/bot/restart",
            &support,
            json!({ "take_over": true }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/bot/restart",
            &owner,
            json!({ "take_over": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"]["problem"], "webhook");
    assert_eq!(body["status"]["detail"], "old.example.com");
    assert!(h.services().bot_runtime().take_takeover());
    let marks = {
        let mut conn = h.db.store.conn().await.unwrap();
        conn.audit()
            .recent_by_action("settings.bot_takeover", 5)
            .await
            .unwrap()
    };
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].details["from"], "old.example.com");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/settings/bot/restart",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/unknown",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/core",
            &owner,
            json!({ "currency": "EUR" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.invalid_body");
    let (status, body) = h.get("/api/admin/modules/gifts/config", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["schema"], json!([]));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/modules/gifts/config",
            &owner,
            json!({ "config": { "anything": 1 } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn тарифы_и_категории_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    h.remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    let (status, category) = h
        .write(
            Method::POST,
            "/api/admin/categories",
            &owner,
            json!({ "name": "Основные" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{category}");
    let tariff = json!({
        "category_id": category["id"],
        "name": "Стандарт",
        "devices": { "included": 3, "extra": { "max": 2, "monthly": { "usd": 99, "rub": 4900 }, "monthly_stars": 50 } },
        "traffic": { "limit_bytes": null, "reset": "no_reset" },
        "squads": [Uuid::from_u128(1)],
        "periods": [
            { "days": 30, "prices": { "usd": 299, "rub": 19900 }, "stars": 150 },
            { "days": 90, "prices": { "usd": 699, "rub": 49900 }, "stars": 350 }
        ]
    });
    let (status, created) = h
        .write(Method::POST, "/api/admin/tariffs", &owner, tariff.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["periods"].as_array().unwrap().len(), 2);
    let id = created["id"].as_i64().unwrap();
    let (status, hidden) = h
        .write(
            Method::PATCH,
            &format!("/api/admin/tariffs/{id}"),
            &owner,
            json!({ "on_sale": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hidden["on_sale"], false);
    let (status, list) = h.get("/api/admin/tariffs", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["tariffs"].as_array().unwrap().len(), 1);
    assert_eq!(list["categories"][0]["name"], "Основные");
    let mut broken = tariff.clone();
    broken["periods"][0]["prices"]["rub"] = json!(19950);
    let (status, body) = h
        .write(Method::POST, "/api/admin/tariffs", &owner, broken)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "tariff.invalid");
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("/api/admin/tariffs/{id}"),
            &owner,
            tariff,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, squads) = h.get("/api/admin/infra/squads", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(squads[0]["tariffs"][0]["name"], "Стандарт");
    h.db.cleanup().await;
}

#[tokio::test]
async fn главная_лента_и_ноды_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let node: Node = serde_json::from_value(json!({
        "uuid": Uuid::from_u128(2), "name": "NL-2", "address": "198.51.100.22",
        "isConnected": false, "isDisabled": false, "isConnecting": false,
        "lastStatusMessage": "connect ETIMEDOUT", "usersOnline": 0
    }))
    .unwrap();
    h.remna.set_nodes(vec![node]);
    h.client(5_093_118_422).await;
    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["remna"]["nodes_total"], 1);
    assert_eq!(dashboard["remna"]["nodes_online"], 0);
    assert_eq!(dashboard["currency"], "USD");
    assert!(dashboard["subscriptions"].is_object() || dashboard["stats_error"].is_string());
    let (status, feed) = h.get("/api/admin/feed?kind=clients&limit=5", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(feed["items"][0]["title"], "Новый клиент");
    let gifted = h
        .services()
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: 5_093_118_499,
                username: None,
                first_name: Some("Дима".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            Some("ref_ABC123__s_tiktok__gift_WRWM-48NG-B662"),
        )
        .await
        .unwrap()
        .0
        .id;
    let (_, feed) = h.get("/api/admin/feed?kind=clients&limit=5", &owner).await;
    assert_eq!(
        feed["items"][0]["data"]["start_param"],
        "ref_ABC123__s_tiktok__gift_"
    );
    assert!(!feed.to_string().contains("WRWM-48NG-B662"));
    let (status, history) = h
        .get(&format!("/api/admin/clients/{}/history", gifted.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert!(history.to_string().contains("ref_ABC123__s_tiktok__gift_"));
    assert!(!history.to_string().contains("WRWM-48NG-B662"));
    let (status, body) = h.get("/api/admin/feed?kind=planets", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.invalid_query");
    let (status, nodes) = h.get("/api/admin/infra/nodes", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nodes["summary"]["attention"], 1);
    assert_eq!(nodes["nodes"][0]["state"], "down");
    let restart = format!(
        "/api/admin/infra/nodes/{}/actions/restart",
        Uuid::from_u128(2)
    );
    let (status, body) = h.write(Method::POST, &restart, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(h.remna.calls().contains(&"restart_node".to_string()));
    let (status, _) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/nodes/{}/actions/explode",
                Uuid::from_u128(2)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = h
        .write(
            Method::POST,
            &format!(
                "/api/admin/infra/nodes/{}/actions/restart",
                Uuid::from_u128(9)
            ),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        h.get("/api/admin/infra/billing", &owner).await.0,
        StatusCode::OK
    );
    let (status, remna) = h.get("/api/admin/remnawave/status", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(remna["ok"], true);
    assert_eq!(h.get("/api/admin/me", &owner).await.1["remnawave"], true);
    let (_, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(dashboard["setup"]["remnawave"], true);
    assert_eq!(dashboard["setup"]["bot"], false);
    assert_eq!(dashboard["setup"]["tariffs"], false);
    h.tariff().await;
    let (_, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(dashboard["setup"]["tariffs"], true);
    h.services().set_remna(None);
    assert_eq!(h.get("/api/admin/me", &owner).await.1["remnawave"], false);
    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dashboard["remna_error"].is_string());
    assert_eq!(dashboard["setup"]["remnawave"], false);
    assert_eq!(
        h.get("/api/admin/infra/nodes", &owner).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn платежи_список_и_ручная_отметка_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_422).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let (status, page) = h.get("/api/admin/payments?status=pending", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (status, body) = h.get("/api/admin/payments?status=lost", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let mark = format!("/api/admin/payments/{}/mark-paid", payment.id.0);
    let (status, body) = h
        .write(Method::POST, &mark, &owner, json!({ "comment": "пришло" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"], "accepted");
    let (status, body) = h.write(Method::POST, &mark, &owner, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "payment.manual_status");
    let (status, page) = h.get("/api/admin/payments?status=paid", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["provider_txid"], Value::Null);
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/refund", payment.id.0),
            &owner,
            json!({ "method": "balance", "subscription_action": "keep" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["refund"]["status"], "refunded");
    assert_eq!(body["refund"]["amount"]["minor"], payment.amount.minor);
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/refund", payment.id.0),
            &owner,
            json!({ "method": "balance", "subscription_action": "keep" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "payment.refund_status");
    let (status, page) = h.get("/api/admin/payments?status=refunded", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    h.db.cleanup().await;
}

#[tokio::test]
async fn возврат_через_http_с_вариантами_ключом_и_вне_панели() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_423).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let refund = format!("/api/admin/payments/{}/refund", payment.id.0);
    let (status, options) = h.get(&refund, &owner).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert!(options["note"].is_string());
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/mark-paid", payment.id.0),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, options) = h.get(&refund, &owner).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["note"], Value::Null);
    assert_eq!(options["remaining"]["minor"], payment.amount.minor);
    assert_eq!(options["to_balance"]["available"], true);
    assert_eq!(options["outside"]["available"], true);
    assert_eq!(options["subscription"], false);
    let key = Uuid::new_v4();
    let once = json!({
        "method": "balance", "subscription_action": "keep", "amount_minor": 100,
        "reason": "часть за перебои", "key": key
    });
    let (status, first) = h.write(Method::POST, &refund, &owner, once.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, second) = h.write(Method::POST, &refund, &owner, once).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(first["refund"]["refund_id"], second["refund"]["refund_id"]);
    let outside_key = Uuid::new_v4();
    let outside = json!({
        "method": "provider", "outside": true, "subscription_action": "keep",
        "amount_minor": payment.amount.minor - 100, "key": outside_key
    });
    let (status, done) = h
        .write(Method::POST, &refund, &owner, outside.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["refund"]["pending"], false);
    assert_eq!(done["refund"]["status"], "refunded");
    let (status, again) = h.write(Method::POST, &refund, &owner, outside).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["refund"]["refund_id"], done["refund"]["refund_id"]);
    let (status, options) = h.get(&refund, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(options["refunded"]["minor"], payment.amount.minor);
    assert!(options["note"].is_string());
    let (status, body) = h
        .write(
            Method::POST,
            &refund,
            &owner,
            json!({ "method": "balance", "subscription_action": "keep", "outside": true }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn второй_фактор_клиента_через_http_и_сброс_в_админке() {
    let Some(h) = http().await else { return };
    h.set_bot().await;
    let client = h.client(5_093_118_422).await;
    let cabinet = h.client_token(client).await;
    let (status, me) = h.get("/api/client/me", &cabinet).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        me["two_factor"],
        json!({ "enabled": false, "enabled_at": null })
    );
    let (status, setup) = h
        .write(
            Method::POST,
            "/api/client/security/totp/setup",
            &cabinet,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(
        setup["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/STEALTHNET:%40morozov_a?secret=")
    );
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/security/totp/enable",
            &cabinet,
            json!({ "secret": secret, "code": totp_code(&secret, start()) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    let (status, security) = h.get("/api/client/security", &cabinet).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(security["enabled"], true);

    let (status, _, started) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let poll = started["poll_token"].as_str().unwrap().to_string();
    let link = started["link"]
        .as_str()
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_string();
    h.services()
        .auth()
        .confirm_telegram_login(
            &link,
            &TelegramProfile {
                id: 5_093_118_422,
                username: Some("morozov_a".into()),
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
        )
        .await
        .unwrap();
    let (status, headers, body) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "totp_required");
    assert!(headers.get(header::SET_COOKIE).is_none());
    let challenge = body["challenge"].as_str().unwrap().to_string();
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/totp",
            None,
            Some(json!({ "challenge": challenge, "code": "12" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.totp_code");
    let later = start() + Duration::seconds(30);
    h.clock.set(later);
    let (status, headers, body) = h
        .send(
            Method::POST,
            "/api/client/auth/totp",
            None,
            Some(json!({ "challenge": challenge, "code": totp_code(&secret, later) })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("sn_client=")
    );
    let second = body["token"].as_str().unwrap().to_string();
    assert_eq!(h.get("/api/client/me", &second).await.0, StatusCode::OK);

    let owner = h.owner().await;
    let (status, card) = h
        .get(&format!("/api/admin/clients/{}", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(card["two_factor"]["enabled"], true);
    let reset = format!("/api/admin/clients/{}/totp/reset", client.0);
    let readonly = h.admin("readonly@example.com", AdminRole::ReadOnly).await;
    let (status, _) = h.write(Method::POST, &reset, &readonly, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h.write(Method::POST, &reset, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.get("/api/client/me", &second).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.get("/api/client/me", &cabinet).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, card) = h
        .get(&format!("/api/admin/clients/{}", client.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(card["two_factor"]["enabled"], false);
    let (status, body) = h.write(Method::POST, &reset, &owner, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "client.totp_not_enabled");
    let (status, history) = h
        .get(
            &format!("/api/admin/clients/{}/history?limit=20", client.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let actions: Vec<&str> = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["action"].as_str())
        .collect();
    assert!(actions.contains(&"client.totp_reset"), "{actions:?}");
    assert!(actions.contains(&"client.totp_enabled"), "{actions:?}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn кабинет_входит_через_telegram_и_выходит() {
    let Some(h) = http().await else { return };
    let (status, _, body) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "bot.not_configured");
    h.set_bot().await;
    let (status, _, start_body) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{start_body}");
    let link = start_body["link"].as_str().unwrap();
    let poll = start_body["poll_token"].as_str().unwrap().to_string();
    let code = link.rsplit('=').next().unwrap().to_string();
    let (status, _, pending) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pending["status"], "pending");
    h.services()
        .auth()
        .confirm_telegram_login(
            &code,
            &TelegramProfile {
                id: 5_093_118_422,
                username: Some("morozov_a".into()),
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
        )
        .await
        .unwrap();
    let (status, headers, confirmed) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(confirmed["status"], "confirmed");
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    assert!(cookie.starts_with("sn_client=") && cookie.contains("SameSite=Lax"));
    let token = confirmed["login"]["token"].as_str().unwrap().to_string();
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["channel"], "cabinet");
    let (status, _, again) = h
        .send(
            Method::POST,
            "/api/client/auth/telegram/status",
            None,
            Some(json!({ "poll_token": poll })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["status"], "expired");
    let (status, _, _) = h
        .send(Method::POST, "/api/client/auth/logout", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.get("/api/client/me", &token).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email",
            None,
            Some(json!({ "email": "alex@example.com" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "auth.email_password_mode");
    let (status, _, body) = h
        .send(
            Method::POST,
            "/api/client/auth/email/verify",
            None,
            Some(json!({ "email": "alex@example.com", "code": "123456" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "auth.code_invalid");
    h.db.cleanup().await;
}

#[tokio::test]
async fn частые_запросы_входа_получают_429() {
    let Some(h) = http().await else { return };
    h.set_bot().await;
    let limit = sn_services::limits::rules::TELEGRAM_START_IP.limit;
    for _ in 0..limit {
        let (status, _, _) = h
            .send(Method::POST, "/api/client/auth/telegram", None, None)
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _, body) = h
        .send(Method::POST, "/api/client/auth/telegram", None, None)
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "auth.rate_limited");
    h.db.cleanup().await;
}

#[tokio::test]
async fn кабинет_видит_свой_платёж_и_обращения() {
    let Some(h) = http().await else { return };
    let client = h.client(5_093_118_422).await;
    let stranger = h.client(42).await;
    let token = h.client_token(client).await;
    let stranger_token = h.client_token(stranger).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let path = format!("/api/client/payments/{}", payment.public_id);
    let (status, body) = h.get(&path, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["amount"]["minor"], 699);
    assert_eq!(body["order_kind"], "new");
    assert_eq!(h.get(&path, &stranger_token).await.0, StatusCode::NOT_FOUND);
    let (status, opened) = h
        .write(
            Method::POST,
            "/api/client/support/tickets",
            &token,
            json!({ "subject": "Чек", "body": "Не пришёл чек об оплате" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    let (status, list) = h.get("/api/client/support/tickets", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["channel"], "cabinet");
    let ticket = list[0]["id"].as_i64().unwrap();
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/client/support/tickets/{ticket}/close"),
            &stranger_token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({ "amount": "десять", "provider": "cryptopay" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "validation.amount");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({ "amount": "0.50", "provider": "cryptopay" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "balance.topup_out_of_range");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/topup",
            &token,
            json!({ "amount": "10.00", "provider": "cryptopay" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "checkout.method_unavailable");
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "trial.disabled");
    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.settings()
            .set(
                keys::TRIAL,
                &serde_json::to_value(sn_domain::settings::TrialSettings::single(
                    true,
                    3,
                    vec![SquadId(Uuid::from_u128(1))],
                    sn_domain::traffic::TrafficPolicy::UNLIMITED,
                    1,
                ))
                .unwrap(),
                None,
            )
            .await
            .unwrap();
    }
    h.services().reload().await.unwrap();
    h.services().set_remna(Some(h.remna.clone()));
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["order_id"].is_i64(), "{body}");
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиенты_ищутся_и_история_собирается_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let first = h.client(5_093_118_422).await;
    let second = h.client(7_000_000_001).await;
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/clients/{}/balance", first.0),
            &owner,
            json!({ "delta": { "minor": 420, "currency": "USD" }, "comment": "подарок", "idempotency_key": "h1" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, page) = h.get("/api/admin/clients", &owner).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let (status, found) = h.get("/api/admin/clients?q=5093118422", &owner).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let items = found["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["client"]["id"], first.0);
    assert_eq!(items[0]["name"], "Алексей");
    assert_eq!(items[0]["balance"]["minor"], 420);
    assert_eq!(items[0]["paid_total"]["minor"], 0);
    let (status, hidden) = h.get("/api/admin/clients?q=5093118422", &support).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hidden["items"][0]["balance"], Value::Null);
    assert_eq!(hidden["items"][0]["paid_total"], Value::Null);
    let (status, none) = h.get("/api/admin/clients?q=нет-такого", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert!(none["items"].as_array().unwrap().is_empty());
    let (status, body) = h.get("/api/admin/clients?segment=lost", &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(page["items"][0]["tariff_name"], Value::Null);
    let (status, counts) = h.get("/api/admin/clients/segments", &support).await;
    assert_eq!(status, StatusCode::OK, "{counts}");
    assert_eq!(counts["total"], 2);
    assert_eq!(counts["no_subscription"], 2);
    assert_eq!(counts["active"], 0);
    let (status, history) = h
        .get(&format!("/api/admin/clients/{}/history", first.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let changed: Vec<&Value> = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "balance_changed")
        .collect();
    assert_eq!(changed.len(), 1, "{history}");
    assert_eq!(changed[0]["event"]["delta"]["minor"], 420);
    let (status, _) = h
        .get(&format!("/api/admin/clients/{}/history", second.0), &owner)
        .await;
    assert_eq!(status, StatusCode::OK);
    h.db.cleanup().await;
}

#[tokio::test]
async fn клиенты_отбираются_по_условиям_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let tariff = h.tariff().await;
    let first = h.client(7_100_000_001).await;
    let second = h.client(7_100_000_002).await;
    {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query(
            "INSERT INTO subscriptions (client_id, tariff_id, kind, expire_at) VALUES ($1, $2, 'paid', $3)",
        )
        .bind(first.0)
        .bind(tariff.id.0)
        .bind(start() + Duration::days(5))
        .execute(conn.conn())
        .await
        .unwrap();
    }
    let ids = |page: &Value| -> Vec<i64> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["client"]["id"].as_i64().unwrap())
            .collect()
    };
    let (status, page) = h
        .get(
            &format!("/api/admin/clients?tariff={}", tariff.id.0),
            &support,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page), vec![first.0]);
    assert_eq!(page["items"][0]["tariff_name"], "Стандарт");
    let (status, page) = h
        .get(
            &format!("/api/admin/clients?tariff={}&status=trial", tariff.id.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(ids(&page).is_empty());
    let (status, page) = h
        .get("/api/admin/clients?ends=7&source=bot&telegram=yes", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page), vec![first.0]);
    let (status, page) = h
        .get("/api/admin/clients?segment=no_subscription", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page), vec![second.0]);
    let (status, page) = h.get("/api/admin/clients?paid=no&balance=no", &owner).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page).len(), 2);
    let (status, counts) = h
        .get(
            &format!(
                "/api/admin/clients/segments?tariff={}&status=trial",
                tariff.id.0
            ),
            &support,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{counts}");
    assert_eq!(counts["total"], 1);
    assert_eq!(counts["active"], 1);
    assert_eq!(counts["expiring_soon"], 0);
    let (status, counts) = h.get("/api/admin/clients/segments?lang=en", &support).await;
    assert_eq!(status, StatusCode::OK, "{counts}");
    assert_eq!(counts["total"], 0);
    for bad in [
        "ends=0",
        "tariff=abc",
        "status=lost",
        "source=ufo",
        "from=2026-11-01",
        "utm=a%20b",
        "paid_from=10&paid_to=5",
    ] {
        let (status, body) = h.get(&format!("/api/admin/clients?{bad}"), &owner).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert_eq!(body["error"]["code"], "validation.filter", "{bad}");
    }
    for money in ["paid=yes", "paid_from=10", "paid_to=10", "balance=no"] {
        let (status, body) = h
            .get(&format!("/api/admin/clients?{money}"), &support)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{money}: {body}");
        let (status, _) = h
            .get(&format!("/api/admin/clients/segments?{money}"), &support)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{money}");
    }
    let (status, options) = h.get("/api/admin/clients/filter-options", &support).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["tariffs"][0]["id"], tariff.id.0);
    assert_eq!(options["tariffs"][0]["name"], "Стандарт");
    assert_eq!(options["tariffs"][0]["deleted"], false);
    assert_eq!(options["categories"][0]["name"], "Основные");
    assert_eq!(options["utm"], json!([]));
    let (status, _) = h
        .get("/api/admin/clients/filter-options", "нет-такого")
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    h.db.cleanup().await;
}

#[tokio::test]
async fn варианты_пробного_периода_через_админку_и_кабинет() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let squad = |n: u128| Uuid::from_u128(n).to_string();
    let unlimited = json!({ "limit_bytes": null, "reset": "no_reset" });
    let (status, trial) = h.get("/api/admin/settings/trial", &owner).await;
    assert_eq!(status, StatusCode::OK, "{trial}");
    assert_eq!(trial["enabled"], false);
    assert_eq!(trial["variants"].as_array().unwrap().len(), 1);
    assert_eq!(trial["variants"][0]["id"], 1);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({
                "enabled": true,
                "variants": [
                    { "id": 1, "name": "Россия", "enabled": true, "days": 3, "devices": 1, "squads": [squad(1)], "traffic": unlimited },
                    { "id": 0, "name": "", "enabled": true, "days": 1, "devices": 2, "squads": [squad(2)], "traffic": unlimited }
                ]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "settings.invalid_field");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("вариант 2: укажите название"),
        "{body}"
    );
    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({
                "enabled": true,
                "variants": [
                    { "id": 1, "name": " Россия ", "enabled": true, "days": 3, "devices": 1, "squads": [squad(1)], "traffic": unlimited },
                    { "id": 0, "name": "Европа", "enabled": true, "days": 1, "devices": 2, "squads": [squad(2)], "traffic": { "limit_bytes": 10_737_418_240u64, "reset": "no_reset" } },
                    { "id": 0, "name": "США", "enabled": false, "days": 7, "devices": 1, "squads": [], "traffic": unlimited }
                ]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let ids: Vec<i64> = saved["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(saved["variants"][0]["name"], "Россия");
    assert_eq!(saved["days"], 3);
    assert_eq!(saved["squads"], json!([squad(1)]));
    h.services().set_remna(Some(h.remna.clone()));
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["trial"], json!({ "enabled": true, "days": 1 }));
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let (status, offer) = h.get("/api/client/trial", &token).await;
    assert_eq!(status, StatusCode::OK, "{offer}");
    assert_eq!(offer["available"], true);
    let names: Vec<&str> = offer["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Россия", "Европа"]);
    assert_eq!(offer["variants"][1]["devices"], 2);
    assert_eq!(
        offer["variants"][1]["traffic"]["limit_bytes"],
        10_737_418_240u64
    );
    assert!(offer["variants"][1]["countries"].is_array());
    assert!(offer["variants"][0].get("squads").is_none(), "{offer}");
    let (status, body) = h
        .write(Method::POST, "/api/client/trial", &token, json!({}))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "trial.variant_required");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "variant": 3 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "trial.variant_unavailable");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "variant": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let order_id = sn_domain::ids::OrderId(body["order_id"].as_i64().unwrap());
    let order = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.orders().get(order_id).await.unwrap()
    };
    assert_eq!(order.snapshot.unwrap().name, "Европа");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/client/trial",
            &token,
            json!({ "variant": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "trial.used");
    let (_, offer) = h.get("/api/client/trial", &token).await;
    assert_eq!(offer, json!({ "available": false, "variants": [] }));
    let (status, old) = h
        .write(
            Method::PUT,
            "/api/admin/settings/trial",
            &owner,
            json!({ "enabled": true, "days": 5, "squads": [squad(1)], "devices": 1, "traffic": unlimited }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{old}");
    assert_eq!(old["variants"].as_array().unwrap().len(), 1);
    assert_eq!(old["variants"][0]["days"], 5);
    h.db.cleanup().await;
}

#[tokio::test]
async fn открытый_каталог_без_входа_и_без_сквадов() {
    let Some(h) = http().await else { return };
    let tariff = h.tariff().await;
    let (status, _, catalog) = h.send(Method::GET, "/api/public/catalog", None, None).await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    assert_eq!(catalog["currency"], "USD");
    let offer = &catalog["tariffs"][0];
    assert_eq!(offer["tariff"]["id"], tariff.id.0);
    assert_eq!(offer["tariff"]["name"], "Стандарт");
    assert_eq!(offer["periods"][0]["price"]["minor"], 699);
    assert!(offer["tariff"].get("squads").is_none());
    assert!(offer["countries"].is_array());
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert!(site["trial"]["days"].is_number());
    assert!(site["autorenew_days_before"].is_number());
    h.db.cleanup().await;
}

async fn public_get(h: &Http, path: &str, etag: Option<&str>) -> (StatusCode, HeaderMap, usize) {
    let mut builder = Request::builder().method(Method::GET).uri(path);
    if let Some(etag) = etag {
        builder = builder.header(header::IF_NONE_MATCH, etag);
    }
    let response = router(h.state.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, body.len())
}

#[tokio::test]
async fn каталог_и_данные_сайта_отдаются_с_etag_и_перепроверкой() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let tariff = h.tariff().await;
    for path in ["/api/public/catalog", "/api/public/site"] {
        let (status, headers, size) = public_get(&h, path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(size > 0);
        assert_eq!(headers[header::CACHE_CONTROL], "no-cache", "{path}");
        let etag = headers[header::ETAG].to_str().unwrap().to_string();
        let (status, headers, size) = public_get(&h, path, Some(&etag)).await;
        assert_eq!(status, StatusCode::NOT_MODIFIED, "{path}");
        assert_eq!(size, 0);
        assert_eq!(headers[header::ETAG].to_str().unwrap(), etag);
        let (status, _, _) = public_get(&h, path, Some("\"old\"")).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    let (_, headers, _) = public_get(&h, "/api/public/catalog", None).await;
    let before = headers[header::ETAG].to_str().unwrap().to_string();
    {
        let mut conn = h.services().store().conn().await.unwrap();
        sqlx::query("UPDATE tariff_periods SET price_usd_minor = 799 WHERE tariff_id = $1")
            .bind(tariff.id.0)
            .execute(conn.conn())
            .await
            .unwrap();
    }
    let (status, headers, _) = public_get(&h, "/api/public/catalog", Some(&before)).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(headers[header::ETAG].to_str().unwrap(), before);
    let (_, _, catalog) = h.send(Method::GET, "/api/public/catalog", None, None).await;
    assert_eq!(catalog["tariffs"][0]["periods"][0]["price"]["minor"], 799);
    let (_, headers, _) = public_get(&h, "/api/public/site", None).await;
    let site_before = headers[header::ETAG].to_str().unwrap().to_string();
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/appearance",
            &owner,
            json!({ "admin": "top", "admin_theme": "system", "client": "mono", "client_theme": "light" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, headers, _) = public_get(&h, "/api/public/site", Some(&site_before)).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(headers[header::ETAG].to_str().unwrap(), site_before);
    h.db.cleanup().await;
}

#[tokio::test]
async fn оформление_сохраняется_и_видно_без_входа() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (status, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        site["appearance"],
        json!({ "admin": "side", "admin_theme": "dark", "client": "accent", "client_theme": "client" })
    );
    assert_eq!(site["currency"], "USD");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/appearance",
            &owner,
            json!({ "admin": "top", "admin_theme": "system", "client": "mono", "client_theme": "light" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["admin"], "top");
    let (_, me) = h.get("/api/admin/me", &owner).await;
    assert_eq!(me["appearance"]["admin"], "top");
    assert_eq!(me["appearance"]["admin_theme"], "system");
    assert!(me["email"].as_str().is_some_and(|e| e.contains('@')));
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["appearance"]["client"], "mono");
    assert_eq!(site["appearance"]["client_theme"], "light");
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/appearance",
            &owner,
            json!({ "admin": "middle" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    h.db.cleanup().await;
}

async fn web_state() -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("sn-web-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>STEALTHNET</title>",
    )
    .unwrap();
    std::fs::write(dir.join("assets/main-1a2b.js"), "console.log(1)").unwrap();
    let mut state = offline_state();
    state.config = Arc::new(ServerConfig {
        web_dir: Some(dir.clone()),
        ..ServerConfig::default()
    });
    (state, dir)
}

async fn fetch(state: &AppState, method: Method, path: &str) -> (StatusCode, HeaderMap, String) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn интерфейс_раздаётся_сервером_с_возвратом_на_index() {
    let (state, dir) = web_state().await;
    let (status, headers, body) = fetch(&state, Method::GET, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (status, headers, body) = fetch(&state, Method::GET, "/admin/clients/42").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (status, headers, _) = fetch(&state, Method::GET, "/app/plans").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::X_FRAME_OPTIONS));
    let (status, headers, body) = fetch(&state, Method::GET, "/assets/main-1a2b.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log(1)");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    let (status, headers, body) = fetch(&state, Method::GET, "/api/nothing-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("json")
    );
    assert!(body.contains("not_found"));
    let (status, _, _) = fetch(&state, Method::POST, "/admin").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn служебный_скрипт_старой_панели_снимается_вместо_index() {
    let (state, dir) = web_state().await;
    for path in ["/sw.js", "/service-worker.js"] {
        let (status, headers, body) = fetch(&state, Method::GET, path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert!(body.contains("skipWaiting"));
        assert!(body.contains("caches.delete"));
        assert!(body.contains("registration.unregister()"));
        assert!(!body.contains("STEALTHNET"));
    }
    std::fs::write(dir.join("sw.js"), "own()").unwrap();
    let (status, _, body) = fetch(&state, Method::GET, "/sw.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "own()");
    let (_, _, body) = fetch(&state, Method::GET, "/other.js").await;
    assert!(body.contains("STEALTHNET"));
    std::fs::remove_dir_all(dir).unwrap();
}

async fn framed(state: &AppState, path: &str, dest: &str) -> (StatusCode, HeaderMap) {
    let request = Request::builder()
        .uri(path)
        .header("sec-fetch-dest", dest)
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    (response.status(), response.headers().clone())
}

#[tokio::test]
async fn во_фрейме_telegram_web_главная_и_кабинет_переходят_в_приложение() {
    let (state, dir) = web_state().await;
    for (path, target) in [
        ("/", "/app"),
        ("/index.html", "/app"),
        ("/cabinet", "/app"),
        ("/cabinet/support/3", "/app"),
        ("/?ref=ABCD1234", "/app?ref=ABCD1234"),
    ] {
        let (status, headers) = framed(&state, path, "iframe").await;
        assert_eq!(status, StatusCode::TEMPORARY_REDIRECT, "{path}");
        assert_eq!(headers[header::LOCATION], target, "{path}");
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert!(!headers.contains_key(header::X_FRAME_OPTIONS), "{path}");
        let (status, headers) = framed(&state, path, "document").await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY", "{path}");
    }
    for path in ["/admin", "/admin/clients/42", "/pay/5", "/cabinets"] {
        let (status, headers) = framed(&state, path, "iframe").await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY", "{path}");
    }
    let (status, headers) = framed(&state, "/app/plans", "iframe").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::X_FRAME_OPTIONS));
    let (status, headers) = framed(&state, "/assets/main-1a2b.js", "iframe").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key(header::LOCATION));
    state
        .services
        .modules()
        .apply_rows(&[sn_store::ModuleStateRow {
            module: "landing".into(),
            enabled: false,
            config: json!({}),
            updated_at: Utc::now(),
        }]);
    let (_, headers) = framed(&state, "/", "iframe").await;
    assert_eq!(headers[header::LOCATION], "/app");
    let (_, headers) = framed(&state, "/", "document").await;
    assert_eq!(headers[header::LOCATION], "/cabinet");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn с_выключенным_приложением_сайт_во_фрейме_по_прежнему_закрыт() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let (mut state, dir) = web_state().await;
    state.services = h.services().clone();
    let (status, _) = framed(&state, "/cabinet", "iframe").await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    let (status, saved) = h
        .write(
            Method::PUT,
            "/api/admin/settings/client_access",
            &owner,
            json!({ "miniapp": false, "site_login": true, "self_devices": true, "self_reissue": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    for path in ["/", "/cabinet"] {
        let (status, headers) = framed(&state, path, "iframe").await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY", "{path}");
        let (_, _, body) = fetch(&state, Method::GET, path).await;
        assert!(!body.contains("telegram-app"), "{path}");
    }
    std::fs::remove_dir_all(dir).unwrap();
    h.db.cleanup().await;
}

#[tokio::test]
async fn выключенный_лендинг_ведёт_с_главной_в_кабинет() {
    let (state, dir) = web_state().await;
    let landing = |enabled: bool| sn_store::ModuleStateRow {
        module: "landing".into(),
        enabled,
        config: json!({}),
        updated_at: Utc::now(),
    };
    state.services.modules().apply_rows(&[landing(false)]);
    let (status, headers, _) = fetch(&state, Method::GET, "/?ref=ABCD1234").await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(headers[header::LOCATION], "/cabinet?ref=ABCD1234");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let (status, headers, _) = fetch(&state, Method::GET, "/index.html").await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(headers[header::LOCATION], "/cabinet");
    let (status, _, body) = fetch(&state, Method::GET, "/cabinet").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    let (status, _, body) = fetch(&state, Method::GET, "/api/public/landing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("module.disabled"));
    state.services.modules().apply_rows(&[landing(true)]);
    let (status, _, body) = fetch(&state, Method::GET, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("STEALTHNET"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn без_каталога_интерфейса_любой_путь_отвечает_json_404() {
    let state = offline_state();
    let (status, headers, _) = fetch(&state, Method::GET, "/admin").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("json")
    );
}

#[tokio::test]
async fn способ_подключения_и_оформления_страницы_remnawave() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    h.remna.set_subpage_configs(vec![
        sn_remnawave::SubpageConfigRef {
            uuid: second,
            name: "Для белых списков".into(),
            view_position: 2,
        },
        sn_remnawave::SubpageConfigRef {
            uuid: first,
            name: "Основное".into(),
            view_position: 1,
        },
    ]);
    let (status, list) = h
        .get("/api/admin/settings/remnawave/page-configs", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list[0]["name"], "Основное");
    assert_eq!(list[1]["uuid"], second.to_string());
    let (_, connect) = h.get("/api/admin/settings/connect", &owner).await;
    assert_eq!(connect, json!({ "mode": "ours", "page_config": null }));
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/connect",
            &owner,
            json!({ "mode": "remnawave", "page_config": second.to_string() }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "remnawave");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["connect"]["page_config"], second.to_string());
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/connect",
            &owner,
            json!({ "mode": "remnawave", "page_config": "не-идентификатор" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn карточка_подписки_подключения_запросы_и_отмена_продлений() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let readonly = h.admin("readonly@example.com", AdminRole::ReadOnly).await;
    let client = h.client(5_093_118_422).await;
    let mut user = sn_remnawave::fake::sample_user(41, "sn_card");
    user.active_internal_squads = vec![sn_remnawave::SquadRef {
        uuid: Uuid::from_u128(7),
        name: "Основной".into(),
    }];
    h.remna.insert_user(user.clone());
    let subscription = {
        let mut tx = h.services().store().begin().await.unwrap();
        let created = tx
            .subscriptions()
            .create(&sn_store::NewSubscription {
                client_id: client,
                tariff_id: None,
                kind: sn_domain::subscription::SubscriptionKind::Paid,
                extra_devices: 0,
                autorenew: sn_domain::subscription::AutoRenewState::default(),
            })
            .await
            .unwrap();
        tx.subscriptions()
            .link_remna(
                created.id,
                &sn_domain::subscription::RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        created.id
    };
    let base = format!("/api/admin/subscriptions/{}", subscription.0);

    let (status, view) = h.get(&base, &readonly).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["squads"], json!(["Основной"]));
    assert_eq!(view["remna_reachable"], true);
    assert_eq!(view["subscription"]["id"], subscription.0);
    assert!(view.get("traffic_30d_bytes").is_some());

    let (status, _) = h
        .write(
            Method::POST,
            &format!("{base}/actions/extend"),
            &readonly,
            json!({ "days": 7 }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, requests) = h
        .write(
            Method::POST,
            &format!("{base}/actions/requests"),
            &readonly,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{requests}");
    assert!(requests.as_array().unwrap().is_empty());
    let (status, connections) = h
        .write(
            Method::POST,
            &format!("{base}/actions/connections"),
            &readonly,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{connections}");
    assert_eq!(connections["state"], "done");
    let (status, bad) = h
        .write(
            Method::POST,
            &format!("{base}/actions/connections"),
            &owner,
            json!({ "job_id": "../../etc" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");

    let before = h.remna.user(user.id).unwrap().expire_at;
    for days in [7, 30] {
        let (status, body) = h
            .write(
                Method::POST,
                &format!("{base}/actions/extend"),
                &owner,
                json!({ "days": days }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, body) = h
        .write(
            Method::POST,
            &format!("{base}/actions/undo-extend"),
            &owner,
            json!({ "count": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(h.remna.user(user.id).unwrap().expire_at, before);

    let (status, body) = h
        .write(
            Method::POST,
            &format!("{base}/actions/limits"),
            &owner,
            json!({ "limits": { "traffic_limit_bytes": 1_073_741_824u64 } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.remna.user(user.id).unwrap().traffic_limit_bytes,
        1_073_741_824
    );
    let (status, body) = h
        .write(
            Method::POST,
            &format!("{base}/actions/limits"),
            &owner,
            json!({ "limits": { "traffic_limit_bytes": null } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(h.remna.user(user.id).unwrap().traffic_limit_bytes, 0);

    let message = format!("/api/admin/clients/{}/message", client.0);
    let (status, body) = h
        .write(
            Method::POST,
            &message,
            &owner,
            json!({ "text": "Здравствуйте!" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "client.bot_missing");
    let (status, _) = h
        .write(
            Method::POST,
            &message,
            &readonly,
            json!({ "text": "Здравствуйте!" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, history) = h
        .get(
            &format!("/api/admin/clients/{}/history?limit=50", client.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let actions: Vec<&str> = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["action"].as_str())
        .collect();
    assert!(actions.contains(&"subscription.add_days"), "{history}");
    assert!(
        actions.contains(&"subscription.add_days_undone"),
        "{history}"
    );
    let undone = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["action"] == "subscription.add_days_undone")
        .unwrap();
    assert_eq!(undone["actor_email"], "owner@example.com");
    assert_eq!(undone["details"]["days"], 37);
    h.db.cleanup().await;
}

#[tokio::test]
async fn выдача_подписки_продление_и_удаление_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    h.remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    let tariff = h.tariff().await;
    let period = tariff.periods[0].id.0;
    let client = h.client(5_093_118_422).await;
    let order_of = |public: Value| {
        let services = h.services().clone();
        async move {
            let public: Uuid = serde_json::from_value(public).unwrap();
            let mut conn = services.store().conn().await.unwrap();
            let id = conn.orders().by_public_id(public).await.unwrap().id;
            drop(conn);
            services.fulfillment().run(id).await.unwrap();
        }
    };

    let (status, options) = h.get("/api/admin/clients/grant-options", &support).await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(options["tariffs"][0]["id"], tariff.id.0);
    assert_eq!(options["tariffs"][0]["periods"][0]["days"], 90);
    assert_eq!(options["tariffs"][0]["ready"], true);

    let grant = format!("/api/admin/clients/{}/grant", client.0);
    let (status, body) = h
        .write(
            Method::POST,
            &grant,
            &support,
            json!({ "tariff_id": tariff.id.0, "days": 0 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "grant.days");
    let (status, granted) = h
        .write(
            Method::POST,
            &grant,
            &support,
            json!({ "tariff_id": tariff.id.0, "period_id": period, "note": "компенсация" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{granted}");
    assert_eq!(granted["plan"], "new");
    assert_eq!(granted["days"], 90);
    order_of(granted["order_id"].clone()).await;

    let card = format!("/api/admin/clients/{}", client.0);
    let (status, body) = h.get(&card, &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let subscription = &body["subscriptions"][0];
    assert_eq!(subscription["kind"], "manual");
    assert_eq!(subscription["tariff_id"], tariff.id.0);
    let sub_id = subscription["id"].as_i64().unwrap();
    let user_id = subscription["remna"]["user_id"].as_i64().unwrap();
    let first_expire = h
        .remna
        .user(sn_domain::ids::RemnaUserId(user_id))
        .unwrap()
        .expire_at;
    assert!(first_expire > Utc::now() + Duration::days(89));

    let (status, renewed) = h
        .write(
            Method::POST,
            &grant,
            &owner,
            json!({ "tariff_id": tariff.id.0, "days": 10, "subscription_id": sub_id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    assert_eq!(renewed["plan"], "renewal");
    order_of(renewed["order_id"].clone()).await;
    let second_expire = h
        .remna
        .user(sn_domain::ids::RemnaUserId(user_id))
        .unwrap()
        .expire_at;
    assert_eq!((second_expire - first_expire).num_days(), 10);

    let (status, history) = h.get(&format!("{card}/history?limit=50"), &owner).await;
    assert_eq!(status, StatusCode::OK);
    let grants = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "order" && item["order"]["kind"] == "admin_grant")
        .count();
    assert_eq!(grants, 2, "{history}");

    let remove = format!("/api/admin/subscriptions/{sub_id}/actions/delete");
    let (status, _) = h.write(Method::POST, &remove, &support, json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h.write(Method::POST, &remove, &owner, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(h.remna.user(sn_domain::ids::RemnaUserId(user_id)).is_none());
    let (_, body) = h.get(&card, &owner).await;
    assert!(body["subscriptions"][0]["ended_at"].is_string(), "{body}");

    let (status, _) = h
        .write(Method::POST, &format!("{card}/delete"), &support, json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(Method::POST, &format!("{card}/delete"), &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = h.get(&card, &owner).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let again = h.client(5_093_118_422).await;
    assert_ne!(again, client);
    h.db.cleanup().await;
}

#[tokio::test]
async fn тарифы_каналы_применение_и_удаление_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    h.remna.set_squads(vec![
        InternalSquad {
            uuid: Uuid::from_u128(1),
            name: "Основной".into(),
            view_position: 0,
            info: SquadInfo::default(),
            inbounds: Vec::new(),
        },
        InternalSquad {
            uuid: Uuid::from_u128(2),
            name: "Резерв".into(),
            view_position: 1,
            info: SquadInfo::default(),
            inbounds: Vec::new(),
        },
    ]);
    let tariff = h.tariff().await;
    let id = tariff.id.0;

    let (status, list) = h.get("/api/admin/tariffs", &owner).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["stats"][0]["tariff_id"], id);
    assert_eq!(list["stats"][0]["sales_30d"], 0);
    assert_eq!(
        list["stats"][0]["channels"],
        json!({ "bot": true, "cabinet": true, "miniapp": true })
    );
    let (status, _) = h.get("/api/admin/tariffs", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, squads) = h.get("/api/admin/tariffs/squads", &owner).await;
    assert_eq!(status, StatusCode::OK, "{squads}");
    assert_eq!(squads[0]["name"], "Основной");
    assert_eq!(squads[1]["name"], "Резерв");

    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let (status, catalog) = h.get("/api/client/catalog", &token).await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    assert_eq!(catalog["tariffs"].as_array().unwrap().len(), 1);
    let channels = format!("/api/admin/tariffs/{id}/channels");
    let (status, body) = h
        .write(
            Method::PUT,
            &channels,
            &owner,
            json!({ "bot": false, "cabinet": false, "miniapp": false }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            &channels,
            &owner,
            json!({ "bot": true, "cabinet": false, "miniapp": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, catalog) = h.get("/api/client/catalog", &token).await;
    assert!(
        catalog["tariffs"].as_array().unwrap().is_empty(),
        "{catalog}"
    );
    let bot_catalog = h
        .services()
        .checkout()
        .catalog_in(Some(sn_domain::access::ClientChannel::Bot))
        .await
        .unwrap();
    assert_eq!(bot_catalog.len(), 1);

    let mut user = sn_remnawave::fake::sample_user(77, "sn_tariff");
    user.active_internal_squads = vec![sn_remnawave::SquadRef {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
    }];
    user.hwid_device_limit = Some(3);
    h.remna.insert_user(user.clone());
    {
        let mut tx = h.services().store().begin().await.unwrap();
        let created = tx
            .subscriptions()
            .create(&sn_store::NewSubscription {
                client_id: client,
                tariff_id: Some(tariff.id),
                kind: sn_domain::subscription::SubscriptionKind::Paid,
                extra_devices: 1,
                autorenew: sn_domain::subscription::AutoRenewState::default(),
            })
            .await
            .unwrap();
        tx.subscriptions()
            .link_remna(
                created.id,
                &sn_domain::subscription::RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let input = json!({
        "category_id": tariff.category_id.0,
        "name": "Стандарт",
        "devices": { "included": 4, "extra": null },
        "traffic": { "limit_bytes": null, "reset": "no_reset" },
        "squads": [Uuid::from_u128(1), Uuid::from_u128(2)],
        "periods": [{ "days": 90, "prices": { "usd": 699, "rub": 49900 }, "stars": 350 }]
    });
    let (status, saved) = h
        .write(
            Method::PUT,
            &format!("/api/admin/tariffs/{id}"),
            &owner,
            input.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["applied"], Value::Null);
    let mut wider = input.clone();
    wider["devices"]["included"] = json!(5);
    wider["squads"] = json!([Uuid::from_u128(2)]);
    let (status, saved) = h
        .write(
            Method::PUT,
            &format!("/api/admin/tariffs/{id}?apply=true"),
            &owner,
            wider,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["applied"]["users"], 1, "{saved}");
    assert_eq!(saved["applied"]["failed"], 0);
    assert_eq!(saved["applied"]["squads"], true);
    assert_eq!(saved["applied"]["devices"], true);
    assert_eq!(saved["applied"]["traffic"], false);
    let updated = h.remna.user(user.id).unwrap();
    assert_eq!(updated.hwid_device_limit, Some(6));
    assert_eq!(updated.squad_ids(), vec![SquadId(Uuid::from_u128(2))]);

    let (status, body) = h
        .write(
            Method::DELETE,
            &format!("/api/admin/tariffs/{id}"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "tariff.in_use");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("На тарифе 1 действующая подписка. "),
        "{body}"
    );
    let (status, usage) = h
        .get(&format!("/api/admin/tariffs/{id}/usage"), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{usage}");
    assert_eq!(
        (usage["active"].clone(), usage["ended"].clone()),
        (json!(1), json!(0))
    );
    assert_eq!(usage["holders"].as_array().unwrap().len(), 1, "{usage}");
    assert_eq!(usage["holders"][0]["client_id"], client.0);
    assert_eq!(usage["holders"][0]["name"], "Алексей");
    assert_eq!(usage["holders"][0]["telegram_username"], "morozov_a");
    assert_eq!(usage["holders"][0]["active"], true);
    let hidden = h
        .services()
        .tariffs()
        .usage(sn_domain::ids::TariffId(id), false)
        .await
        .unwrap();
    assert_eq!((hidden.active, hidden.holders.len()), (1, 0));
    assert_eq!(
        h.get("/api/admin/tariffs/987654/usage", &owner).await.0,
        StatusCode::NOT_FOUND
    );
    let category = tariff.category_id.0;
    let (status, body) = h
        .write(
            Method::DELETE,
            &format!("/api/admin/categories/{category}"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "category.in_use");
    let (status, empty) = h
        .write(
            Method::POST,
            "/api/admin/categories",
            &owner,
            json!({ "name": "Пустая" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    let (status, _) = h
        .write(
            Method::DELETE,
            &format!("/api/admin/categories/{}", empty["id"]),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = h.get("/api/admin/tariffs", &owner).await;
    assert_eq!(list["categories"].as_array().unwrap().len(), 1, "{list}");
    h.db.cleanup().await;
}

#[tokio::test]
async fn лимит_подписок_категории_и_скидка_за_устройства_через_http() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    h.remna.set_squads(vec![InternalSquad {
        uuid: Uuid::from_u128(1),
        name: "Основной".into(),
        view_position: 0,
        info: SquadInfo::default(),
        inbounds: Vec::new(),
    }]);
    let (status, category) = h
        .write(
            Method::POST,
            "/api/admin/categories",
            &owner,
            json!({ "name": "С обходом", "subscription_limit": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{category}");
    assert_eq!(category["subscription_limit"], 1);
    let id = category["id"].as_i64().unwrap();
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/categories/{id}"),
            &owner,
            json!({ "name": "С обходом", "subscription_limit": 0 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "tariff.invalid");
    let (status, updated) = h
        .write(
            Method::PUT,
            &format!("/api/admin/categories/{id}"),
            &owner,
            json!({ "name": "Обход", "sort": 1, "visible": true, "subscription_limit": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["subscription_limit"], 2);
    assert_eq!(updated["name"], "Обход");
    let tariff = |discounts: Value| {
        json!({
            "category_id": id,
            "name": "Обход",
            "devices": { "included": 3, "extra": { "max": 5, "monthly": { "usd": 99, "rub": 4900 }, "monthly_stars": 50 } },
            "traffic": { "limit_bytes": null, "reset": "no_reset" },
            "squads": [Uuid::from_u128(1)],
            "periods": [{ "days": 30, "prices": { "usd": 299, "rub": 19900 }, "stars": 150 }],
            "device_discounts": discounts
        })
    };
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/tariffs",
            &owner,
            tariff(json!([{ "from": 6, "percent": 10 }])),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/tariffs",
            &owner,
            tariff(json!([{ "from": 5, "percent": 20 }, { "from": 2, "percent": 10 }])),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(
        created["device_discounts"],
        json!([{ "from": 2, "percent": 10 }, { "from": 5, "percent": 20 }])
    );
    let (_, list) = h.get("/api/admin/tariffs", &owner).await;
    assert_eq!(list["categories"][0]["subscription_limit"], 2, "{list}");
    let client = h.client(5_093_118_501).await;
    let token = h.client_token(client).await;
    let (status, catalog) = h.get("/api/client/catalog", &token).await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    assert_eq!(catalog["tariffs"][0]["category"]["subscription_limit"], 2);
    assert_eq!(
        catalog["tariffs"][0]["tariff"]["device_discounts"][1]["percent"],
        20
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn главная_сводка_лента_оплаты_и_обращения_с_именами() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let tariff = h.tariff().await;
    let payment = h.pending_payment(client, &tariff).await;
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/payments/{}/mark-paid", payment.id.0),
            &owner,
            json!({ "comment": "пришло" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, opened) = h
        .write(
            Method::POST,
            "/api/client/support/tickets",
            &token,
            json!({ "subject": "Не подключается", "body": "Не подключается на iPhone после обновления" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{opened}");

    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["today"]["payments"], 1);
    assert_eq!(dashboard["today"]["new_clients"], 1);
    assert_eq!(dashboard["today"]["new_clients_paid"], 1);
    assert_eq!(dashboard["revenue"]["today"]["minor"], 699);
    assert_eq!(dashboard["revenue"]["by_day"].as_array().unwrap().len(), 14);
    assert_eq!(dashboard["support_waiting"], 1);

    let (status, feed) = h
        .get("/api/admin/feed?kind=payments&limit=10", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{feed}");
    let paid = feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "payment_paid")
        .expect("оплата в ленте");
    assert_eq!(paid["client"]["name"], "Алексей");
    assert_eq!(paid["client"]["telegram_username"], "morozov_a");
    assert_eq!(paid["order"]["tariff_name"], "Стандарт");
    assert_eq!(paid["order"]["days"], 90);
    assert_eq!(paid["payment"]["amount"]["minor"], 699);

    let (status, payments) = h.get("/api/admin/payments?limit=5", &owner).await;
    assert_eq!(status, StatusCode::OK, "{payments}");
    assert_eq!(payments["items"][0]["client"]["name"], "Алексей");
    assert_eq!(payments["items"][0]["order"]["tariff_name"], "Стандарт");
    assert_eq!(payments["items"][0]["status"], "paid");

    let (status, tickets) = h
        .get("/api/admin/support/tickets?filter=waiting&limit=3", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{tickets}");
    assert_eq!(tickets["counts"]["waiting"], 1);
    assert_eq!(tickets["items"][0]["client_name"], "Алексей");
    assert_eq!(
        tickets["items"][0]["preview"],
        "Не подключается на iPhone после обновления"
    );
    assert!(tickets["items"][0]["waiting_since"].is_string());
    h.db.cleanup().await;
}

#[tokio::test]
async fn поддержка_поиск_прочтение_ответ_и_повторное_открытие() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let (status, opened) = h
        .write(
            Method::POST,
            "/api/client/support/tickets",
            &token,
            json!({ "subject": "Не подключается", "body": "Happ пишет timeout после обновления" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{opened}");

    let (status, list) = h
        .get("/api/admin/support/tickets?filter=waiting", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let id = list["items"][0]["ticket"]["id"].as_i64().unwrap();
    assert_eq!(list["items"][0]["unread"], true);
    assert_eq!(list["items"][0]["last_author"], "client");
    for q in ["timeout", "алексей", "подключ"] {
        let (status, found) = h
            .get(
                &format!(
                    "/api/admin/support/tickets?filter=waiting&q={}",
                    urlencoding(q)
                ),
                &owner,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{found}");
        assert_eq!(found["items"].as_array().unwrap().len(), 1, "{q}: {found}");
    }
    let (_, none) = h
        .get(
            "/api/admin/support/tickets?filter=waiting&q=nothing",
            &owner,
        )
        .await;
    assert!(none["items"].as_array().unwrap().is_empty());

    let thread = format!("/api/admin/support/tickets/{id}");
    let (status, body) = h.get(&thread, &owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h
        .get("/api/admin/support/tickets?filter=waiting", &owner)
        .await;
    assert_eq!(list["items"][0]["unread"], false);

    let (status, replied) = h
        .write(
            Method::POST,
            &thread,
            &owner,
            json!({ "body": "Обновите подписку", "close": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replied}");
    assert_eq!(replied["ticket"]["status"], "closed");
    assert_eq!(replied["admins"][0]["email"], "owner@example.com");
    let (_, counts) = h
        .get("/api/admin/support/tickets?filter=closed", &owner)
        .await;
    assert_eq!(counts["counts"]["closed"], 1);
    assert_eq!(counts["counts"]["waiting"], 0);

    let (status, _) = h
        .write(Method::POST, &format!("{thread}/reopen"), &owner, json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = h.get(&thread, &owner).await;
    assert_eq!(body["ticket"]["status"], "open");
    let (_, mine) = h.get("/api/client/support/tickets", &token).await;
    assert_eq!(mine[0]["id"], id);
    h.db.cleanup().await;
}

fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[tokio::test]
async fn инфраструктура_ноды_создание_город_сброс_и_удаление() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let inbound = Uuid::from_u128(31);
    let profile = Uuid::from_u128(30);
    h.remna
        .set_config_profiles(vec![sn_remnawave::ConfigProfile {
            uuid: profile,
            name: "Основной профиль".into(),
            view_position: 0,
            inbounds: vec![sn_remnawave::Inbound {
                uuid: inbound,
                profile_uuid: Some(profile),
                tag: "VLESS_REALITY".into(),
                kind: "vless".into(),
                network: Some("tcp".into()),
                security: Some("reality".into()),
                port: Some(443),
            }],
            nodes: Vec::new(),
        }]);
    let (status, profiles) = h.get("/api/admin/infra/profiles", &owner).await;
    assert_eq!(status, StatusCode::OK, "{profiles}");
    assert_eq!(profiles[0]["inbounds"][0]["port"], 443);
    let (status, _) = h.get("/api/admin/infra/nodes", &support).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/infra/nodes",
            &owner,
            json!({ "name": " ", "address": "198.51.100.35", "profile_uuid": profile, "inbounds": [inbound] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "node.name");
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/infra/nodes",
            &owner,
            json!({
                "name": "NL-3", "address": "198.51.100.35", "country_code": "nl", "city": "Амстердам",
                "profile_uuid": profile, "inbounds": [inbound]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["port"], 2222);
    assert_eq!(created["secret_key"], "fake-node-secret-key");
    let uuid = created["node"]["uuid"].as_str().unwrap().to_string();

    let (status, list) = h.get("/api/admin/infra/nodes", &owner).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let node = list["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["node"]["uuid"] == uuid.as_str())
        .expect("новая нода в списке");
    assert_eq!(node["city"], "Амстердам");
    assert_eq!(node["node"]["countryCode"], "NL");
    assert_eq!(node["inbounds"][0]["tag"], "VLESS_REALITY");
    assert_eq!(node["state"], "down");
    assert!(list["summary"]["attention"].as_u64().unwrap() >= 1);

    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/nodes/{uuid}"),
            &owner,
            json!({ "name": "NL", "address": "198.51.100.36" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "node.name");
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/nodes/{uuid}"),
            &owner,
            json!({ "name": "DE-3", "address": " 198.51.100.36 ", "country_code": "de", "city": "Франкфурт" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "DE-3");
    let (_, list) = h.get("/api/admin/infra/nodes", &owner).await;
    let node = list["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["node"]["uuid"] == uuid.as_str())
        .expect("нода после правки");
    assert_eq!(node["node"]["address"], "198.51.100.36");
    assert_eq!(node["node"]["countryCode"], "DE");
    assert_eq!(node["city"], "Франкфурт");
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/nodes/{uuid}"),
            &support,
            json!({ "name": "DE-4", "address": "198.51.100.36" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/infra/nodes/{uuid}/actions/reset-traffic"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(h.remna.calls().contains(&"reset_node_traffic".to_string()));
    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/infra/nodes/{uuid}/actions/delete"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = h.get("/api/admin/infra/nodes", &owner).await;
    assert!(
        list["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["node"]["uuid"] != uuid.as_str())
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn инфраструктура_сквады_провайдеры_и_оплата_серверов() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let profile = Uuid::from_u128(40);
    let inbound = Uuid::from_u128(41);
    h.remna
        .set_config_profiles(vec![sn_remnawave::ConfigProfile {
            uuid: profile,
            name: "Основной профиль".into(),
            view_position: 0,
            inbounds: vec![sn_remnawave::Inbound {
                uuid: inbound,
                profile_uuid: Some(profile),
                tag: "VLESS_REALITY".into(),
                kind: "vless".into(),
                network: Some("tcp".into()),
                security: Some("reality".into()),
                port: Some(443),
            }],
            nodes: Vec::new(),
        }]);

    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/infra/squads",
            &owner,
            json!({ "name": "Europe", "inbounds": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "squad.inbounds");
    let (status, body) = h
        .write(
            Method::POST,
            "/api/admin/infra/squads",
            &owner,
            json!({ "name": "Европа", "inbounds": [inbound] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "squad.name");
    let (status, squad) = h
        .write(
            Method::POST,
            "/api/admin/infra/squads",
            &owner,
            json!({ "name": "Europe", "inbounds": [inbound] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{squad}");
    let squad_uuid = squad["uuid"].as_str().unwrap().to_string();
    let squad_id = Uuid::parse_str(&squad_uuid).unwrap();
    let other_squad = Uuid::from_u128(49);
    let host = |n: u128, remark: &str, disabled: bool| sn_remnawave::Host {
        uuid: Uuid::from_u128(n),
        view_position: n as i64,
        remark: remark.into(),
        address: format!("{n}.example.com"),
        port: 443,
        is_disabled: disabled,
        is_hidden: false,
        inbound: sn_remnawave::HostInbound {
            config_profile_uuid: Some(profile),
            config_profile_inbound_uuid: Some(inbound),
        },
        nodes: Vec::new(),
        tags: Vec::new(),
        server_description: None,
        internal_squads: None,
        ..Default::default()
    };
    let mut excluded = host(46, "Финляндия", false);
    excluded.internal_squads = Some(sn_remnawave::HostSquadFilter::exclude(vec![
        squad_id,
        other_squad,
    ]));
    h.remna.set_hosts(vec![
        host(44, "Нидерланды", false),
        host(45, "Германия", true),
        excluded,
    ]);
    let (status, hosts) = h.get("/api/admin/infra/hosts", &owner).await;
    assert_eq!(status, StatusCode::OK, "{hosts}");
    assert_eq!(hosts["hosts"].as_array().unwrap().len(), 3);
    assert_eq!(hosts["hosts"][0]["host"]["remark"], "Нидерланды");

    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/squads/{squad_uuid}"),
            &owner,
            json!({ "name": "Europe 2", "inbounds": [inbound], "hosts": [{ "uuid": Uuid::from_u128(44), "remark": " " }] }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "host.remark");
    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/squads/{squad_uuid}"),
            &owner,
            json!({ "name": "Europe 2", "inbounds": [inbound], "hosts": [{ "uuid": Uuid::from_u128(99), "shown": false }] }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, renamed) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/squads/{squad_uuid}"),
            &owner,
            json!({ "name": "Europe-2", "inbounds": [inbound], "hosts": [
                { "uuid": Uuid::from_u128(44), "remark": " Нидерланды, резерв ", "shown": false },
                { "uuid": Uuid::from_u128(45), "remark": "Германия", "shown": true },
                { "uuid": Uuid::from_u128(46), "shown": true }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "Europe-2");
    let hosts = h.remna.host_list();
    let find = |n: u128| hosts.iter().find(|x| x.uuid == Uuid::from_u128(n)).unwrap();
    assert_eq!(find(44).remark, "Нидерланды, резерв");
    assert_eq!(
        find(44).internal_squads,
        Some(sn_remnawave::HostSquadFilter::exclude(vec![squad_id]))
    );
    assert!(!find(44).shown_to(squad_id));
    assert!(find(44).shown_to(other_squad));
    assert!(!find(45).is_disabled);
    assert_eq!(
        find(45).internal_squads,
        Some(sn_remnawave::HostSquadFilter::allow_only(vec![squad_id]))
    );
    assert!(find(45).shown_to(squad_id));
    assert!(!find(45).shown_to(other_squad));
    assert!(find(46).shown_to(squad_id));
    assert!(!find(46).shown_to(other_squad));
    let updates = h
        .remna
        .calls()
        .iter()
        .filter(|c| c.as_str() == "update_host")
        .count();
    assert_eq!(updates, 3);

    let (status, body) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/squads/{squad_uuid}"),
            &owner,
            json!({ "name": "Europe-2", "inbounds": [inbound], "hosts": [
                { "uuid": Uuid::from_u128(44), "remark": "Нидерланды, резерв", "shown": true },
                { "uuid": Uuid::from_u128(45), "shown": true }
            ] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hosts = h.remna.host_list();
    let find = |n: u128| hosts.iter().find(|x| x.uuid == Uuid::from_u128(n)).unwrap();
    assert!(find(44).shown_to(squad_id));
    assert_eq!(
        find(44).internal_squads,
        Some(sn_remnawave::HostSquadFilter::exclude(Vec::new()))
    );
    assert_eq!(
        h.remna
            .calls()
            .iter()
            .filter(|c| c.as_str() == "update_host")
            .count(),
        updates + 1
    );

    let (status, provider) = h
        .write(
            Method::POST,
            "/api/admin/infra/providers",
            &owner,
            json!({ "name": "Hetzner", "login_url": "https://accounts.hetzner.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{provider}");
    let provider_uuid = provider["uuid"].as_str().unwrap().to_string();
    let (status, bad) = h
        .write(
            Method::POST,
            "/api/admin/infra/providers",
            &owner,
            json!({ "name": "X", "login_url": "ftp://x" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");

    let due = (Utc::now() + Duration::days(2)).to_rfc3339();
    let (status, created) = h
        .write(
            Method::POST,
            "/api/admin/infra/nodes",
            &owner,
            json!({ "name": "FI-1", "address": "203.0.113.30", "country_code": "FI", "profile_uuid": profile,
                    "inbounds": [inbound], "provider_uuid": provider_uuid, "next_billing_at": due }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let node_uuid = created["node"]["uuid"].as_str().unwrap().to_string();
    let (status, _) = h
        .write(
            Method::PUT,
            &format!("/api/admin/infra/prices/{node_uuid}"),
            &owner,
            json!({ "amount_minor": 759 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, view) = h.get("/api/admin/infra/billing/view", &owner).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["rows"][0]["node_name"], "FI-1");
    assert_eq!(view["rows"][0]["price"]["minor"], 759);
    assert_eq!(view["month_total"]["minor"], 759);
    assert_eq!(view["providers"][0]["nodes"], 1);
    assert!(view["revenue_30d"].is_object());
    let billing_uuid = view["rows"][0]["uuid"].as_str().unwrap().to_string();
    let before = view["rows"][0]["next_billing_at"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, body) = h
        .write(
            Method::POST,
            &format!("/api/admin/infra/billing/{billing_uuid}/paid"),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, view) = h.get("/api/admin/infra/billing/view", &owner).await;
    let after = view["rows"][0]["next_billing_at"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(before, after);
    assert!(
        h.remna
            .calls()
            .contains(&"create_infra_billing_record".to_string())
    );
    assert_eq!(view["history"][0]["provider"], "Hetzner");
    assert_eq!(view["history"][0]["nodes"], json!(["FI-1"]));
    assert_eq!(view["history"][0]["amount"]["minor"], 759);
    let (status, history) = h
        .get("/api/admin/infra/billing/history?start=0&size=10", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history["total"], 1);
    assert_eq!(history["records"][0]["nodes"], json!(["FI-1"]));
    h.db.cleanup().await;
}

fn logo_png(width: u32, height: u32, fill: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes.extend_from_slice(&[fill; 64]);
    bytes
}

async fn upload_logo(
    h: &Http,
    token: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/settings/logo")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

async fn raw(state: &AppState, path: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

#[tokio::test]
async fn логотип_загружается_раздаётся_заменяется_и_удаляется() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, _) = upload_logo(&h, &support, "image/png", logo_png(256, 256, 1)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = upload_logo(&h, &owner, "text/plain", logo_png(256, 256, 1)).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>".to_vec();
    let (status, body) = upload_logo(&h, &owner, "image/png", svg).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "logo.format");
    let (status, body) = upload_logo(&h, &owner, "image/png", logo_png(16, 16, 1)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "logo.too_small");
    let mut heavy = logo_png(256, 256, 1);
    heavy.resize(sn_domain::image::LOGO_MAX_BYTES + 1, 0);
    let (status, body) = upload_logo(&h, &owner, "image/png", heavy).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "logo.too_heavy");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["logo_url"], Value::Null);

    let first = logo_png(256, 256, 1);
    let (status, logo) = upload_logo(&h, &owner, "image/png", first.clone()).await;
    assert_eq!(status, StatusCode::OK, "{logo}");
    let url = logo["url"].as_str().unwrap().to_string();
    assert!(
        url.starts_with("/api/public/logo/") && url.ends_with(".png"),
        "{url}"
    );
    assert_eq!(logo["width"], 256);
    assert_eq!(logo["content_type"], "image/png");
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["logo_url"], url.as_str());
    let (_, me) = h.get("/api/admin/me", &owner).await;
    assert_eq!(me["logo_url"], url.as_str());
    let (status, headers, bytes) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, first);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let (status, _, _) = raw(&h.state, &url.replace(".png", ".jpg")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let second = logo_png(512, 512, 2);
    let (status, replaced) = upload_logo(&h, &owner, "image/png", second.clone()).await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    let next = replaced["url"].as_str().unwrap().to_string();
    assert_ne!(next, url);
    let (status, _, _) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, bytes) = raw(&h.state, &next).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, second);

    let (status, _) = h
        .write(
            Method::DELETE,
            "/api/admin/settings/logo",
            &support,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::DELETE,
            "/api/admin/settings/logo",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], true);
    let (_, _, site) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(site["logo_url"], Value::Null);
    let (status, _, _) = raw(&h.state, &next).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = h
        .write(
            Method::DELETE,
            "/api/admin/settings/logo",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(body["removed"], false);
    let marks = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.audit()
            .recent_by_action("settings.logo", 10)
            .await
            .unwrap()
    };
    assert_eq!(marks.len(), 3);
    h.db.cleanup().await;
}

#[tokio::test]
async fn описание_сайта_настраивается_и_попадает_в_страницу() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let (status, site) = h.get("/api/admin/settings/site", &owner).await;
    assert_eq!(status, StatusCode::OK, "{site}");
    assert_eq!(site["title"], Value::Null);
    assert_eq!(site["indexing"], true);
    assert_eq!(site["brand_name"], "STEALTHNET");
    assert_eq!(site["title_max"], 70);
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/site",
            &support,
            json!({ "title": "Чужой" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/site",
            &owner,
            json!({ "title": "x".repeat(71) }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = h
        .write(
            Method::PUT,
            "/api/admin/settings/site",
            &owner,
            json!({ "title": "  Тихий\nVPN  ", "description": "Быстрый & стабильный <VPN>", "indexing": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["title"], "Тихий VPN");
    assert_eq!(body["indexing"], false);
    let (_, _, public) = h.send(Method::GET, "/api/public/site", None, None).await;
    assert_eq!(public["title"], "Тихий VPN");
    assert_eq!(public["brand_name"], "STEALTHNET");

    {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.settings()
            .set(keys::PUBLIC_URL, &json!("https://vpn.example"), None)
            .await
            .unwrap();
    }
    h.services().reload().await.unwrap();
    let dir = std::env::temp_dir().join(format!("sn-web-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html>\n<html lang=\"ru\">\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"robots\" content=\"noindex\" />\n    <title>STEALTHNET</title>\n  </head>\n  <body><div id=\"root\"></div></body>\n</html>\n",
    )
    .unwrap();
    let state = AppState {
        config: Arc::new(ServerConfig {
            web_dir: Some(dir.clone()),
            ..ServerConfig::default()
        }),
        services: h.state.services.clone(),
    };
    let (status, _, page) = fetch(&state, Method::GET, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("<title>Тихий VPN</title>"), "{page}");
    assert!(page.contains("<meta name=\"robots\" content=\"noindex\">"));
    assert!(
        page.contains(
            "<meta name=\"description\" content=\"Быстрый &amp; стабильный &lt;VPN&gt;\">"
        )
    );
    assert!(page.contains("<meta property=\"og:url\" content=\"https://vpn.example/\">"));
    assert!(
        page.contains("<link rel=\"icon\" type=\"image/svg+xml\" href=\"/api/public/icon.svg\">")
    );
    assert!(!page.contains("canonical"));
    let (status, _) = h
        .write(
            Method::PUT,
            "/api/admin/settings/site",
            &owner,
            json!({ "indexing": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, logo) = upload_logo(&h, &owner, "image/png", logo_png(128, 128, 7)).await;
    let logo_url = logo["url"].as_str().unwrap().to_string();
    let (_, _, page) = fetch(&state, Method::GET, "/").await;
    assert!(page.contains("<meta name=\"robots\" content=\"index, follow\">"));
    assert!(page.contains("<link rel=\"canonical\" href=\"https://vpn.example/\">"));
    assert!(page.contains(&format!(
        "<meta property=\"og:image\" content=\"https://vpn.example{logo_url}\">"
    )));
    let (_, headers, admin_page) = fetch(&state, Method::GET, "/admin/settings").await;
    assert!(admin_page.contains("<title>Админка — STEALTHNET</title>"));
    assert!(admin_page.contains("noindex, nofollow"));
    assert!(!admin_page.contains("description"));
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    let (_, _, cabinet) = fetch(&state, Method::GET, "/cabinet").await;
    assert!(cabinet.contains("<title>Личный кабинет — STEALTHNET</title>"));
    let (status, headers, icon) = fetch(&state, Method::GET, "/api/public/icon.svg").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    assert!(icon.contains(">S</text>"));
    std::fs::remove_dir_all(dir).unwrap();
    h.db.cleanup().await;
}

fn favicon_ico(side: u8) -> Vec<u8> {
    let mut dib = 40u32.to_le_bytes().to_vec();
    dib.resize(40 + usize::from(side) * usize::from(side) * 4, 0);
    let mut bytes = vec![0, 0, 1, 0, 1, 0, side, side, 0, 0, 1, 0, 32, 0];
    bytes.extend_from_slice(&u32::try_from(dib.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&22u32.to_le_bytes());
    bytes.extend_from_slice(&dib);
    bytes
}

async fn upload_favicon(
    h: &Http,
    token: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/admin/settings/favicon")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap();
    let response = router(h.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn фавикон_загружается_отдельно_и_подменяет_логотип_во_вкладке() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;

    let (status, headers, body) = raw(&h.state, "/favicon.ico").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=3600");
    assert!(String::from_utf8_lossy(&body).contains(">S</text>"));

    let (status, _) = upload_favicon(&h, &support, "image/x-icon", favicon_ico(32)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = upload_favicon(&h, &owner, "image/jpeg", favicon_ico(32)).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
    assert_eq!(body["error"]["code"], "favicon.format");
    let (status, body) = upload_favicon(&h, &owner, "image/png", logo_png(64, 32, 1)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "favicon.not_square");
    let (status, body) = upload_favicon(&h, &owner, "image/png", logo_png(8, 8, 1)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "favicon.too_small");
    let mut heavy = logo_png(256, 256, 1);
    heavy.resize(sn_domain::image::FAVICON_MAX_BYTES + 1, 0);
    let (status, body) = upload_favicon(&h, &owner, "image/png", heavy).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "favicon.too_heavy");
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_vec();
    let (status, body) = upload_favicon(&h, &owner, "image/png", svg).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["code"], "favicon.format");

    let logo = logo_png(256, 256, 3);
    let (status, logo_view) = upload_logo(&h, &owner, "image/png", logo.clone()).await;
    assert_eq!(status, StatusCode::OK, "{logo_view}");
    let (status, headers, body) = raw(&h.state, "/favicon.ico").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/png");
    assert_eq!(body, logo);

    let icon = favicon_ico(32);
    let (status, view) = upload_favicon(&h, &owner, "image/vnd.microsoft.icon", icon.clone()).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let url = view["url"].as_str().unwrap().to_string();
    assert!(
        url.starts_with("/api/public/favicon/") && url.ends_with(".ico"),
        "{url}"
    );
    assert_eq!(view["width"], 32);
    assert_eq!(view["content_type"], "image/x-icon");
    let (status, headers, body) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "image/x-icon");
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(body, icon);
    let (_, _, body) = raw(&h.state, "/favicon.ico").await;
    assert_eq!(body, icon);
    let (status, site) = h.get("/api/admin/settings/site", &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(site["favicon"]["url"], url.as_str());
    assert_eq!(site["logo"]["url"], logo_view["url"]);

    let dir = std::env::temp_dir().join(format!("sn-web-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><title>STEALTHNET</title></head><body></body></html>",
    )
    .unwrap();
    let state = AppState {
        config: Arc::new(ServerConfig {
            web_dir: Some(dir.clone()),
            ..ServerConfig::default()
        }),
        services: h.state.services.clone(),
    };
    for path in ["/", "/cabinet", "/app", "/admin/clients"] {
        let (_, _, page) = fetch(&state, Method::GET, path).await;
        assert!(
            page.contains(&format!(
                "<link rel=\"icon\" type=\"image/x-icon\" href=\"{url}\">"
            )),
            "{path}: {page}"
        );
        assert!(
            page.contains(&format!(
                "<link rel=\"apple-touch-icon\" href=\"{}\">",
                logo_view["url"].as_str().unwrap()
            )),
            "{path}: {page}"
        );
    }

    let png_icon = logo_png(180, 180, 9);
    let (status, replaced) = upload_favicon(&h, &owner, "image/png", png_icon.clone()).await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    let next = replaced["url"].as_str().unwrap().to_string();
    let (status, _, _) = raw(&h.state, &url).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, page) = fetch(&state, Method::GET, "/admin").await;
    assert!(page.contains(&format!("<link rel=\"apple-touch-icon\" href=\"{next}\">")));

    let (status, body) = h
        .write(
            Method::DELETE,
            "/api/admin/settings/favicon",
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], true);
    let (_, _, body) = raw(&h.state, "/favicon.ico").await;
    assert_eq!(body, logo);
    let (status, _, _) = raw(&h.state, &next).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let marks = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.audit()
            .recent_by_action("settings.favicon", 10)
            .await
            .unwrap()
    };
    assert_eq!(marks.len(), 3);
    std::fs::remove_dir_all(dir).unwrap();
    h.db.cleanup().await;
}

mod system_updates;
