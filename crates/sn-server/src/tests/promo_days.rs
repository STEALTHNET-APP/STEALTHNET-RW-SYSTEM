use super::*;
use sn_domain::ids::{RemnaUserId, SubscriptionId};
use sn_domain::module::ModuleId;
use sn_domain::subscription::{
    AutoRenewState, RemnaLink, RemnaMirror, SubscriptionKind, SubscriptionStatus,
};
use sn_remnawave::fake::sample_user;
use sn_store::NewSubscription;

async fn with_promo() -> Option<Http> {
    let db = test_store().await?;
    let clock = Arc::new(FixedClock::new(start()));
    let services = Services::new(ServiceDeps {
        store: db.store.clone(),
        payments: Arc::new(ProviderRegistry::builtin()),
        modules: Arc::new(ModuleRegistry::new(sn_modules::catalog::all()).unwrap()),
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

async fn linked(h: &Http, client: ClientId, remna: i64, username: &str) -> SubscriptionId {
    let user = sample_user(remna, username);
    h.remna.insert_user(user.clone());
    let mut conn = h.services().store().conn().await.unwrap();
    let subscription = conn
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
        .link_remna(
            subscription.id,
            &RemnaLink {
                user_id: RemnaUserId(remna),
                username: username.into(),
                short_uuid: Some(user.short_uuid.clone()),
            },
        )
        .await
        .unwrap();
    conn.subscriptions()
        .apply_mirror(
            subscription.id,
            &RemnaMirror {
                status: Some(SubscriptionStatus::Active),
                expire_at: Some(user.expire_at),
                ..RemnaMirror::default()
            },
        )
        .await
        .unwrap();
    subscription.id
}

async fn expire_at(h: &Http, token: &str, id: SubscriptionId) -> DateTime<Utc> {
    let (status, view) = h
        .get(&format!("/api/client/subscriptions/{}", id.0), token)
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    serde_json::from_value(view["subscription"]["mirror"]["expire_at"].clone()).unwrap()
}

async fn promo(h: &Http, body: Value) {
    sn_modules::catalog::promo::create(
        &h.services().module_context(),
        &serde_json::from_value(body).unwrap(),
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn кабинет_активирует_промокод_на_дни_и_видит_новый_срок() {
    let Some(h) = with_promo().await else { return };
    let client = h.client(5_093_118_901).await;
    let first = linked(&h, client, 301, "morozov_a").await;
    let second = linked(&h, client, 302, "morozov_a_2").await;
    promo(&h, json!({ "code": "WEEK7", "kind": "days", "days": 7 })).await;
    promo(&h, json!({ "code": "MONTH30", "kind": "days", "days": 30 })).await;
    promo(
        &h,
        json!({ "code": "SALE20", "kind": "percent", "percent": 20 }),
    )
    .await;
    let token = h.client_token(client).await;

    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["promo_codes"], true);

    let (status, found) = h
        .write(
            Method::POST,
            "/api/client/m/promo/check",
            &token,
            json!({ "code": "sale20" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["discount"], true);
    assert_eq!(found["type"], "percent");
    assert_eq!(found["percent"], 20);
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/m/promo/activate",
            &token,
            json!({ "code": "SALE20", "subscription_id": first.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "promo.discount_code");

    let (status, found) = h
        .write(
            Method::POST,
            "/api/client/m/promo/check",
            &token,
            json!({ "code": " week7 " }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["discount"], false);
    assert_eq!(found["type"], "days");
    assert_eq!(found["days"], 7);
    assert_eq!(found["code"], "WEEK7");

    let untouched = expire_at(&h, &token, first).await;
    let before = expire_at(&h, &token, second).await;
    let (status, done) = h
        .write(
            Method::POST,
            "/api/client/m/promo/activate",
            &token,
            json!({ "code": "WEEK7", "subscription_id": second.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        done,
        json!({ "code": "WEEK7", "days": 7, "subscription_id": second.0 })
    );
    assert_eq!(
        expire_at(&h, &token, second).await,
        before + Duration::days(7)
    );
    assert_eq!(expire_at(&h, &token, first).await, untouched);
    assert_eq!(h.remna.calls_of("extend_user").len(), 1);

    let (status, again) = h
        .write(
            Method::POST,
            "/api/client/m/promo/activate",
            &token,
            json!({ "code": "WEEK7", "subscription_id": first.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{again}");
    assert_eq!(again["error"]["code"], "promo.already_used");
    assert_eq!(expire_at(&h, &token, first).await, untouched);

    let stranger = h.client(5_093_118_902).await;
    let foreign = h.client_token(stranger).await;
    let (status, refused) = h
        .write(
            Method::POST,
            "/api/client/m/promo/activate",
            &foreign,
            json!({ "code": "MONTH30", "subscription_id": first.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["error"]["code"], "promo.no_subscription");
    assert_eq!(expire_at(&h, &token, first).await, untouched);
    assert_eq!(h.remna.calls_of("extend_user").len(), 1);

    h.services()
        .modules()
        .set_enabled(h.services().store(), ModuleId::Promo, false, None)
        .await
        .unwrap();
    let (_, me) = h.get("/api/client/me", &token).await;
    assert_eq!(me["promo_codes"], false);
    let (status, off) = h
        .write(
            Method::POST,
            "/api/client/m/promo/activate",
            &token,
            json!({ "code": "MONTH30", "subscription_id": first.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{off}");
    assert_eq!(off["error"]["code"], "module.disabled");
    h.db.cleanup().await;
}

#[tokio::test]
async fn кабинет_скрывает_докупку_когда_модуль_доп_опций_выключен() {
    let Some(h) = with_promo().await else { return };
    let client = h.client(5_093_118_902).await;
    let token = h.client_token(client).await;

    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["options"], true);

    h.services()
        .modules()
        .set_enabled(h.services().store(), ModuleId::Options, false, None)
        .await
        .unwrap();
    let (status, me) = h.get("/api/client/me", &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["options"], false);
    h.db.cleanup().await;
}
