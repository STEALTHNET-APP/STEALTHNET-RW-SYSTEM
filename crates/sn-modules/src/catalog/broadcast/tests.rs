use super::*;
use crate::catalog::testing;
use crate::context::{BalanceChange, CoreApi, Notification};
use crate::registry::ModuleRegistry;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sn_domain::access::{AdminPrincipal, AdminRole};
use sn_domain::client::{IdentityKind, NewIdentity};
use sn_domain::jobs::JobPayload;
use sn_domain::money::{Currency, Money};
use sn_store::Store;
use sn_store::testing::{TestDb, test_store};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tower::ServiceExt;

#[derive(Debug, Clone)]
pub(super) struct Sent {
    pub(super) chat: i64,
    pub(super) html: String,
    photo: Option<String>,
    pub(super) buttons: Vec<OutButton>,
    at: Instant,
}

pub(super) struct FakeSender {
    bot: Option<i64>,
    delay: Duration,
    log: Mutex<Vec<Sent>>,
    script: Mutex<HashMap<i64, VecDeque<Delivery>>>,
}

impl FakeSender {
    pub(super) fn new() -> Arc<Self> {
        Self::slow(Duration::ZERO)
    }

    fn slow(delay: Duration) -> Arc<Self> {
        Arc::new(FakeSender {
            bot: Some(77),
            delay,
            log: Mutex::new(Vec::new()),
            script: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn answer(&self, chat: i64, deliveries: Vec<Delivery>) {
        self.script
            .lock()
            .unwrap()
            .entry(chat)
            .or_default()
            .extend(deliveries);
    }

    pub(super) fn sent(&self) -> Vec<Sent> {
        self.log.lock().unwrap().clone()
    }

    pub(super) fn chats(&self) -> Vec<i64> {
        let mut chats: Vec<i64> = self.sent().into_iter().map(|s| s.chat).collect();
        chats.sort_unstable();
        chats
    }
}

#[async_trait]
impl BotSender for FakeSender {
    fn bot_id(&self) -> Option<i64> {
        self.bot
    }

    async fn send(&self, chat_id: i64, message: &Outgoing<'_>) -> Delivery {
        let at = Instant::now();
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        self.log.lock().unwrap().push(Sent {
            chat: chat_id,
            html: message.html.to_string(),
            photo: message.photo.map(|p| match p {
                Photo::Cached(file) => format!("cached:{file}"),
                Photo::Upload { content_type, .. } => format!("upload:{content_type}"),
            }),
            buttons: message.buttons.to_vec(),
            at,
        });
        let scripted = self
            .script
            .lock()
            .unwrap()
            .get_mut(&chat_id)
            .and_then(VecDeque::pop_front);
        scripted.unwrap_or_else(|| Delivery::Sent {
            file_id: matches!(message.photo, Some(Photo::Upload { .. })).then(|| "F-1".into()),
        })
    }
}

struct Core {
    sender: Option<Arc<dyn BotSender>>,
    mail: Option<Arc<dyn MailSender>>,
    held: Arc<AtomicBool>,
}

#[async_trait]
impl CoreApi for Core {
    async fn change_balance(&self, _change: BalanceChange) -> Result<Money, ModuleError> {
        Err(ModuleError::NotImplemented("change_balance"))
    }

    async fn add_days(
        &self,
        _subscription_id: sn_domain::ids::SubscriptionId,
        _days: u32,
        _reason: &str,
    ) -> Result<(), ModuleError> {
        Err(ModuleError::NotImplemented("add_days"))
    }

    async fn notify(
        &self,
        _client_id: ClientId,
        _notification: Notification,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn enqueue(&self, _job: JobPayload) -> Result<(), ModuleError> {
        Ok(())
    }

    fn bot_sender(&self) -> Option<Arc<dyn BotSender>> {
        self.sender.clone()
    }

    fn mail_sender(&self) -> Option<Arc<dyn MailSender>> {
        self.mail.clone()
    }

    fn core_settings(&self) -> Arc<sn_domain::settings::CoreSettings> {
        let mut settings = sn_domain::settings::CoreSettings::default();
        settings.maintenance.enabled = self.held.load(Ordering::SeqCst);
        Arc::new(settings)
    }
}

fn context(store: &Store, sender: Option<Arc<FakeSender>>) -> ModuleContext {
    context_with(store, sender, None)
}

pub(super) fn context_with(
    store: &Store,
    sender: Option<Arc<FakeSender>>,
    mail: Option<Arc<FakeMail>>,
) -> ModuleContext {
    held_context(store, sender, mail, Arc::new(AtomicBool::new(false)))
}

pub(super) fn held_context(
    store: &Store,
    sender: Option<Arc<FakeSender>>,
    mail: Option<Arc<FakeMail>>,
    held: Arc<AtomicBool>,
) -> ModuleContext {
    ModuleContext {
        store: store.clone(),
        core: Arc::new(Core {
            sender: sender.map(|s| s as Arc<dyn BotSender>),
            mail: mail.map(|m| m as Arc<dyn MailSender>),
            held,
        }),
        currency: Currency::Usd,
    }
}

#[derive(Debug, Clone)]
pub(super) struct Mailed {
    pub(super) to: String,
    subject: String,
    html: String,
    text: String,
    pub(super) unsubscribe: Option<String>,
    at: Instant,
}

pub(super) struct FakeMail {
    pub(super) ready: Mutex<Result<MailSite, String>>,
    log: Mutex<Vec<Mailed>>,
    script: Mutex<HashMap<String, VecDeque<MailDelivery>>>,
    fallback: Mutex<Option<MailDelivery>>,
}

impl FakeMail {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(FakeMail {
            ready: Mutex::new(Ok(MailSite {
                base_url: "https://vpn.example.com".into(),
                brand: "Тест & Ко".into(),
            })),
            log: Mutex::new(Vec::new()),
            script: Mutex::new(HashMap::new()),
            fallback: Mutex::new(None),
        })
    }

    fn answer(&self, to: &str, deliveries: Vec<MailDelivery>) {
        self.script
            .lock()
            .unwrap()
            .entry(to.to_string())
            .or_default()
            .extend(deliveries);
    }

    pub(super) fn mailed(&self) -> Vec<Mailed> {
        self.log.lock().unwrap().clone()
    }
}

#[async_trait]
impl MailSender for FakeMail {
    fn site(&self) -> Result<MailSite, String> {
        self.ready.lock().unwrap().clone()
    }

    async fn send(&self, mail: &MailOut<'_>) -> MailDelivery {
        self.log.lock().unwrap().push(Mailed {
            to: mail.to.to_string(),
            subject: mail.subject.to_string(),
            html: mail.html.to_string(),
            text: mail.text.to_string(),
            unsubscribe: mail.unsubscribe.map(str::to_string),
            at: Instant::now(),
        });
        let scripted = self
            .script
            .lock()
            .unwrap()
            .get_mut(mail.to)
            .and_then(VecDeque::pop_front);
        scripted
            .or_else(|| self.fallback.lock().unwrap().clone())
            .unwrap_or(MailDelivery::Sent)
    }
}

pub(super) fn fast() -> EngineConfig {
    EngineConfig {
        per_second: 500,
        chat_gap: Duration::from_millis(400),
        in_flight: 4,
        idle: Duration::from_millis(40),
        retries: 3,
        retry_pause: Duration::from_millis(40),
        bot_pause: Duration::from_millis(200),
        lease: Duration::from_secs(2),
        grace: Duration::from_secs(2),
        reject_limit: 3,
    }
}

pub(super) struct Running {
    stop: oneshot::Sender<()>,
    handle: JoinHandle<()>,
}

impl Running {
    pub(super) async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.handle)
            .await
            .expect("движок не остановился")
            .unwrap();
    }
}

pub(super) fn spawn_engine(
    ctx: &ModuleContext,
    sender: &Arc<FakeSender>,
    config: EngineConfig,
) -> Running {
    let (stop, stopped) = oneshot::channel::<()>();
    let engine = Engine::new(ctx.clone(), Sender::Bot(sender.clone()), || true, config);
    let handle = tokio::spawn(engine.run(async move {
        let _ = stopped.await;
    }));
    Running { stop, handle }
}

pub(super) async fn wait_status(store: &Store, id: i64, wanted: Status) -> Broadcast {
    let started = Instant::now();
    loop {
        let mut conn = store.conn().await.unwrap();
        let view = store::view(&mut conn, id).await.unwrap();
        drop(conn);
        if view.status == wanted {
            return view;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "рассылка {id} не дошла до {wanted:?}: {view:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(super) async fn tg_client(store: &Store, chat: i64) -> ClientId {
    let id = testing::client(store).await;
    let mut conn = store.conn().await.unwrap();
    conn.clients()
        .add_identity(
            id,
            &NewIdentity {
                kind: IdentityKind::Telegram,
                value: chat.to_string(),
                verified: true,
                meta: serde_json::json!({ "first_name": "Тест" }),
            },
        )
        .await
        .unwrap();
    id
}

struct Sub {
    kind: &'static str,
    days: i64,
    connected: bool,
    tariff: Option<TariffId>,
    ended: bool,
}

const PAID: Sub = Sub {
    kind: "paid",
    days: 30,
    connected: true,
    tariff: None,
    ended: false,
};

async fn subscribe(store: &Store, client: ClientId, sub: Sub) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO subscriptions (client_id, tariff_id, kind, expire_at, first_connected_at, ended_at) \
         VALUES ($1, $2, $3, now() + make_interval(days => $4::int), CASE WHEN $5 THEN now() END, \
         CASE WHEN $6 THEN now() END)",
    )
    .bind(client.0)
    .bind(sub.tariff.map(|t| t.0))
    .bind(sub.kind)
    .bind(i32::try_from(sub.days).unwrap())
    .bind(sub.connected)
    .bind(sub.ended)
    .execute(conn.conn())
    .await
    .unwrap();
}

async fn tariff(store: &Store, name: &str) -> TariffId {
    let mut conn = store.conn().await.unwrap();
    let category: i64 =
        sqlx::query_scalar("INSERT INTO tariff_categories (name) VALUES ('Обычные') RETURNING id")
            .fetch_one(conn.conn())
            .await
            .unwrap();
    TariffId(
        sqlx::query_scalar("INSERT INTO tariffs (category_id, name) VALUES ($1, $2) RETURNING id")
            .bind(category)
            .bind(name)
            .fetch_one(conn.conn())
            .await
            .unwrap(),
    )
}

async fn exec(store: &Store, sql: &str, id: i64) {
    let mut conn = store.conn().await.unwrap();
    sqlx::query(sql)
        .bind(id)
        .execute(conn.conn())
        .await
        .unwrap();
}

pub(super) fn draft(text: &str, audience: Audience) -> Draft {
    Draft {
        channel: Channel::Bot,
        subject: None,
        text: text.into(),
        image_id: None,
        buttons: Vec::new(),
        audience,
    }
}

pub(super) fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

pub(super) fn code(err: BroadcastError) -> &'static str {
    match err {
        BroadcastError::Rejected { code, .. } => code,
        BroadcastError::Module(err) => panic!("ожидался отказ, а пришла ошибка {err}"),
    }
}

async fn clients(store: &Store, from: i64, count: i64) -> Vec<ClientId> {
    let mut out = Vec::new();
    for chat in from..from + count {
        out.push(tg_client(store, chat).await);
    }
    out
}

#[test]
fn адреса_кнопок_проверяются_и_дополняются() {
    assert_eq!(
        clean_url("t.me/stealth").as_deref(),
        Some("https://t.me/stealth")
    );
    assert_eq!(
        clean_url("@stealth").as_deref(),
        Some("https://t.me/stealth")
    );
    assert_eq!(
        clean_url(" https://example.com/a?b=1#c ").as_deref(),
        Some("https://example.com/a?b=1#c")
    );
    assert_eq!(
        clean_url("tg://resolve?domain=stealth").as_deref(),
        Some("tg://resolve?domain=stealth")
    );
    for bad in [
        "",
        "example.com",
        "javascript:alert(1)",
        "https://",
        "https:///path",
        "https://exa mple.com",
        "https://ex\"ample.com",
        "data:text/html,1",
    ] {
        assert_eq!(clean_url(bad), None, "{bad}");
    }
    assert_eq!(
        clean_url(&format!("https://e.com/{}", "a".repeat(URL_MAX))),
        None
    );
}

#[test]
fn кнопки_нормализуются_и_проверяются() {
    let buttons = clean_buttons(&[
        ButtonSpec {
            text: "  Наш   канал ".into(),
            url: Some("t.me/news".into()),
            section: None,
        },
        ButtonSpec {
            text: "Продлить".into(),
            url: Some("  ".into()),
            section: Some(BotSection::Buy),
        },
    ])
    .unwrap();
    assert_eq!(buttons[0].text, "Наш канал");
    assert_eq!(buttons[0].url.as_deref(), Some("https://t.me/news"));
    assert_eq!(buttons[1].url, None);
    assert_eq!(buttons[1].out().callback.as_deref(), Some("n:buy"));
    let both = clean_buttons(&[ButtonSpec {
        text: "Две цели".into(),
        url: Some("https://a.ru".into()),
        section: Some(BotSection::Menu),
    }]);
    assert!(both.is_err());
    let empty = clean_buttons(&[ButtonSpec {
        text: " ".into(),
        url: Some("https://a.ru".into()),
        section: None,
    }]);
    assert!(
        matches!(empty, Err(BroadcastError::Rejected { message, .. }) if message.starts_with("Кнопка 1"))
    );
    let many: Vec<ButtonSpec> = (0..=BUTTONS_MAX)
        .map(|i| ButtonSpec {
            text: format!("К{i}"),
            url: None,
            section: Some(BotSection::Menu),
        })
        .collect();
    assert!(clean_buttons(&many).is_err());
    for section in BotSection::ALL {
        assert!(section.callback().len() <= 64, "{section:?}");
    }
}

#[test]
fn текст_проверяется_по_правилам_telegram() {
    assert_eq!(
        clean_text("  <b>Привет</b> & пока  ", false).unwrap(),
        "<b>Привет</b> &amp; пока"
    );
    assert!(clean_text("   ", false).is_err());
    assert!(clean_text("<b>не закрыт", false).is_err());
    assert!(clean_text("<a href=\"{link}\">x</a>", false).is_err());
    let long = "а".repeat(CAPTION_MAX + 1);
    assert!(clean_text(&long, false).is_ok());
    let err = clean_text(&long, true).unwrap_err();
    assert!(err.to_string().contains("1024"));
    assert!(clean_text(&"а".repeat(TEXT_MAX + 1), false).is_err());
    assert_eq!(
        store::preview("<b>Скидка  20%</b> &amp; подарок\n\nПодробности ниже"),
        "Скидка 20% & подарок"
    );
    assert_eq!(store::preview("\n\n  <i>Привет</i>  "), "Привет");
}

#[test]
fn аудитория_пишется_и_читается_одинаково() {
    let audience: Audience =
        serde_json::from_value(serde_json::json!({ "kind": "expiring", "days": 3 })).unwrap();
    assert_eq!(audience, Audience::Expiring { days: 3 });
    assert_eq!(
        serde_json::to_value(Audience::Tariff {
            tariff_id: TariffId(5)
        })
        .unwrap(),
        serde_json::json!({ "kind": "tariff", "tariff_id": 5 })
    );
    assert!(Audience::Expiring { days: 0 }.check().is_err());
    assert!(Audience::Expiring { days: DAYS_MAX + 1 }.check().is_err());
    assert_eq!(
        Audience::Expiring { days: 1 }.describe(None, None),
        "Подписка закончится в течение 1 дня"
    );
    assert_eq!(
        Audience::Expiring { days: 5 }.describe(None, None),
        "Подписка закончится в течение 5 дней"
    );
}

#[test]
fn картинка_проверяется() {
    assert!(check_image(&png(800, 600)).is_ok());
    assert_eq!(
        code(check_image(&png(50, 600)).unwrap_err()),
        "broadcast.image"
    );
    assert!(check_image(&png(9000, 2000)).is_err());
    assert!(check_image(&png(100, 2100)).is_err());
    assert!(check_image(b"GIF89a").is_err());
    assert!(check_image(&[]).is_err());
}

#[tokio::test]
async fn черновик_сохраняется_правится_и_удаляется() {
    let Some(db) = test_store().await else {
        return;
    };
    let ctx = context(&db.store, None);
    let bad = create(&ctx, &draft("<i>оборвано", Audience::All), None)
        .await
        .unwrap_err();
    assert_eq!(code(bad), "broadcast.invalid");
    let image = upload_image(&ctx, &png(640, 480), None).await.unwrap();
    assert_eq!((image.width, image.height), (640, 480));
    let mut with_image = draft(&"б".repeat(CAPTION_MAX + 5), Audience::All);
    with_image.image_id = Some(image.id);
    assert_eq!(
        code(create(&ctx, &with_image, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    let mut ok = draft("Новые <b>серверы</b> в Нидерландах", Audience::Active);
    ok.image_id = Some(image.id);
    ok.buttons = vec![ButtonSpec {
        text: "Подробнее".into(),
        url: Some("t.me/news".into()),
        section: None,
    }];
    let created = create(&ctx, &ok, None).await.unwrap();
    assert_eq!(created.status, Status::Draft);
    assert_eq!(created.image.as_ref().map(|i| i.id), Some(image.id));
    assert_eq!(created.buttons[0].url.as_deref(), Some("https://t.me/news"));
    assert_eq!(created.audience_label, "С активной подпиской");
    let mut changed = ok.clone();
    changed.text = "Другой текст".into();
    changed.audience = Audience::Expiring { days: 3 };
    let updated = update(&ctx, created.id, &changed).await.unwrap();
    assert_eq!(updated.text, "Другой текст");
    assert_eq!(
        updated.audience_label,
        "Подписка закончится в течение 3 дней"
    );
    let gone = draft(
        "x",
        Audience::Tariff {
            tariff_id: TariffId(999),
        },
    );
    assert_eq!(
        code(update(&ctx, created.id, &gone).await.unwrap_err()),
        "broadcast.invalid"
    );
    let listed = list(&ctx, PageRequest::default()).await.unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].preview, "Другой текст");
    assert!(listed.items[0].has_image);
    delete(&ctx, created.id).await.unwrap();
    assert!(
        list(&ctx, PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(cleanup(&ctx).await.unwrap(), 0);
    exec(
        &db.store,
        "UPDATE broadcast_images SET created_at = now() - interval '2 days' WHERE id = $1",
        image.id,
    )
    .await;
    assert_eq!(cleanup(&ctx).await.unwrap(), 1);
    db.cleanup().await;
}

#[tokio::test]
async fn аудитории_совпадают_с_ожиданием() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context(store, None);
    let pro = tariff(store, "Про").await;
    let paid = tg_client(store, 1001).await;
    subscribe(store, paid, PAID).await;
    let trial = tg_client(store, 1002).await;
    subscribe(
        store,
        trial,
        Sub {
            kind: "trial",
            days: 2,
            connected: false,
            ..PAID
        },
    )
    .await;
    let expired = tg_client(store, 1003).await;
    subscribe(store, expired, Sub { days: -3, ..PAID }).await;
    tg_client(store, 1004).await;
    let soon = tg_client(store, 1005).await;
    subscribe(
        store,
        soon,
        Sub {
            days: 2,
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    let stopped = tg_client(store, 1006).await;
    subscribe(
        store,
        stopped,
        Sub {
            connected: false,
            ..PAID
        },
    )
    .await;
    exec(
        store,
        "UPDATE clients SET bot_unreachable = true WHERE id = $1",
        stopped.0,
    )
    .await;
    let mail = testing::client(store).await;
    subscribe(store, mail, PAID).await;
    let blocked = tg_client(store, 1008).await;
    exec(
        store,
        "UPDATE clients SET status = 'blocked' WHERE id = $1",
        blocked.0,
    )
    .await;
    let deleted = tg_client(store, 1009).await;
    exec(
        store,
        "UPDATE clients SET deleted_at = now() WHERE id = $1",
        deleted.0,
    )
    .await;
    let ended = tg_client(store, 1010).await;
    subscribe(
        store,
        ended,
        Sub {
            days: -120,
            ended: true,
            ..PAID
        },
    )
    .await;
    let both = tg_client(store, 1011).await;
    subscribe(store, both, Sub { days: -10, ..PAID }).await;
    subscribe(store, both, PAID).await;
    let count = |audience| {
        let ctx = ctx.clone();
        async move {
            let reach = reach(&ctx, audience).await.unwrap();
            (reach.recipients, reach.stopped)
        }
    };
    assert_eq!(count(Audience::All).await, (7, 1));
    assert_eq!(count(Audience::Active).await, (3, 1));
    assert_eq!(count(Audience::Expiring { days: 3 }).await, (2, 0));
    assert_eq!(count(Audience::Expiring { days: 60 }).await, (4, 1));
    assert_eq!(count(Audience::Expired).await, (2, 0));
    assert_eq!(count(Audience::NeverConnected).await, (1, 1));
    assert_eq!(count(Audience::Trial).await, (1, 0));
    assert_eq!(count(Audience::NoSubscription).await, (1, 0));
    assert_eq!(count(Audience::Tariff { tariff_id: pro }).await, (1, 0));

    let mut conn = store.conn().await.unwrap();
    for (client, email) in [
        (paid, "Paid@Example.com"),
        (trial, "trial@example.com"),
        (mail, "mail@example.com"),
        (expired, "gone@example.com"),
        (deleted, "deleted@example.com"),
    ] {
        conn.clients()
            .add_identity(
                client,
                &NewIdentity {
                    kind: IdentityKind::Email,
                    value: email.into(),
                    verified: true,
                    meta: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
    }
    drop(conn);
    exec(
        store,
        "INSERT INTO broadcast_unsubscribes (email, client_id) VALUES ('gone@example.com', $1)",
        expired.0,
    )
    .await;
    let mails = |audience| {
        let ctx = ctx.clone();
        async move {
            let reach = reach(&ctx, audience).await.unwrap();
            (reach.emails, reach.unsubscribed)
        }
    };
    assert_eq!(mails(Audience::All).await, (3, 1));
    assert_eq!(mails(Audience::Active).await, (2, 0));
    assert_eq!(mails(Audience::Trial).await, (1, 0));
    assert_eq!(mails(Audience::Expired).await, (0, 1));
    assert_eq!(mails(Audience::NoSubscription).await, (0, 0));
    assert_eq!(count(Audience::All).await, (7, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn закончившаяся_подписка_определяется_по_главной_подписке() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let ctx = context(store, None);
    let latest = |status: &str| {
        format!(
            "UPDATE subscriptions SET status = '{status}' \
             WHERE id = (SELECT max(id) FROM subscriptions WHERE client_id = $1)"
        )
    };
    let no_date = "UPDATE subscriptions SET expire_at = NULL \
                   WHERE id = (SELECT max(id) FROM subscriptions WHERE client_id = $1)";
    let missing = "UPDATE subscriptions SET remna_missing_at = now() WHERE client_id = $1";

    let only_expired = tg_client(store, 4001).await;
    subscribe(store, only_expired, Sub { days: -3, ..PAID }).await;
    let ended = tg_client(store, 4002).await;
    subscribe(
        store,
        ended,
        Sub {
            days: -40,
            ended: true,
            ..PAID
        },
    )
    .await;
    let disabled = tg_client(store, 4003).await;
    subscribe(store, disabled, Sub { days: -5, ..PAID }).await;
    subscribe(store, disabled, Sub { days: 10, ..PAID }).await;
    exec(store, &latest("disabled"), disabled.0).await;
    let limited = tg_client(store, 4004).await;
    subscribe(store, limited, Sub { days: 10, ..PAID }).await;
    exec(store, &latest("limited"), limited.0).await;
    let undated = tg_client(store, 4005).await;
    subscribe(store, undated, PAID).await;
    exec(store, no_date, undated.0).await;
    let undated_and_expired = tg_client(store, 4006).await;
    subscribe(store, undated_and_expired, PAID).await;
    exec(store, no_date, undated_and_expired.0).await;
    subscribe(store, undated_and_expired, Sub { days: -1, ..PAID }).await;
    let gone = tg_client(store, 4007).await;
    subscribe(store, gone, PAID).await;
    exec(store, missing, gone.0).await;
    let gone_and_active = tg_client(store, 4008).await;
    subscribe(store, gone_and_active, Sub { days: -3, ..PAID }).await;
    exec(store, missing, gone_and_active.0).await;
    subscribe(store, gone_and_active, PAID).await;
    let marked_expired = tg_client(store, 4009).await;
    subscribe(store, marked_expired, PAID).await;
    exec(store, &latest("expired"), marked_expired.0).await;
    let marked_active = tg_client(store, 4010).await;
    subscribe(store, marked_active, Sub { days: -3, ..PAID }).await;
    exec(store, &latest("active"), marked_active.0).await;
    tg_client(store, 4011).await;
    let ended_and_active = tg_client(store, 4012).await;
    subscribe(
        store,
        ended_and_active,
        Sub {
            days: -40,
            ended: true,
            ..PAID
        },
    )
    .await;
    subscribe(store, ended_and_active, PAID).await;

    let mut conn = store.conn().await.unwrap();
    for (client, email) in [
        (only_expired, "first@example.com"),
        (disabled, "disabled@example.com"),
        (gone, "gone@example.com"),
    ] {
        conn.clients()
            .add_identity(
                client,
                &NewIdentity {
                    kind: IdentityKind::Email,
                    value: email.into(),
                    verified: true,
                    meta: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
    }
    drop(conn);

    let reach = reach(&ctx, Audience::Expired).await.unwrap();
    assert_eq!((reach.recipients, reach.stopped), (5, 0));
    assert_eq!((reach.emails, reach.unsubscribed), (2, 0));

    let created = create(&ctx, &draft("Возвращайтесь", Audience::Expired), None)
        .await
        .unwrap();
    let started = start(&ctx, created.id, None, None).await.unwrap();
    assert_eq!(started.progress.total, 5);
    let mut conn = store.conn().await.unwrap();
    let recipients: Vec<i64> = sqlx::query_scalar(
        "SELECT client_id FROM broadcast_recipients WHERE broadcast_id = $1 ORDER BY client_id",
    )
    .bind(created.id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    let mut expected = vec![
        only_expired.0,
        ended.0,
        undated_and_expired.0,
        gone.0,
        marked_expired.0,
    ];
    expected.sort_unstable();
    assert_eq!(recipients, expected);
    db.cleanup().await;
}

#[tokio::test]
async fn рассылка_уходит_всем_и_отмечает_остановивших_бота() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    let people = clients(store, 2001, 6).await;
    sender.answer(
        2003,
        vec![Delivery::Blocked("Клиент остановил бота".into())],
    );
    sender.answer(
        2005,
        vec![Delivery::Failed {
            error: "Telegram отказал (400): Bad Request: что-то не так".into(),
            retry: false,
        }],
    );
    let mut message = draft("<b>Скидка</b> 20%", Audience::All);
    message.buttons = vec![
        ButtonSpec {
            text: "Купить".into(),
            url: None,
            section: Some(BotSection::Buy),
        },
        ButtonSpec {
            text: "Канал".into(),
            url: Some("https://t.me/news".into()),
            section: None,
        },
    ];
    let created = create(&ctx, &message, None).await.unwrap();
    let started = start(&ctx, created.id, None, None).await.unwrap();
    assert_eq!(started.status, Status::Sending);
    assert_eq!(started.progress.total, 6);
    assert_eq!(started.progress.pending, 6);
    assert_eq!(
        code(update(&ctx, created.id, &message).await.unwrap_err()),
        "broadcast.state"
    );
    let engine = spawn_engine(&ctx, &sender, fast());
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!(
        done.progress,
        Progress {
            total: 6,
            sent: 4,
            failed: 1,
            blocked: 1,
            skipped: 0,
            pending: 0,
        }
    );
    assert_eq!(done.problems.len(), 1);
    assert_eq!(done.problems[0].count, 1);
    assert_eq!(sender.chats(), (2001..=2006).collect::<Vec<_>>());
    let first = &sender.sent()[0];
    assert_eq!(first.html, "<b>Скидка</b> 20%");
    assert_eq!(first.photo, None);
    assert_eq!(first.buttons[0].callback.as_deref(), Some("n:buy"));
    assert_eq!(first.buttons[1].url.as_deref(), Some("https://t.me/news"));
    let mut conn = store.conn().await.unwrap();
    assert!(conn.clients().get(people[2]).await.unwrap().bot_unreachable);
    assert!(!conn.clients().get(people[0]).await.unwrap().bot_unreachable);
    drop(conn);
    let again = reach(&ctx, Audience::All).await.unwrap();
    assert_eq!((again.recipients, again.stopped), (5, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn ответ_429_ставит_всех_на_паузу_и_повторяет_без_дублей() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    clients(store, 3001, 4).await;
    sender.answer(3002, vec![Delivery::Wait(Duration::from_millis(1200))]);
    let created = create(&ctx, &draft("Новости", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let engine = spawn_engine(
        &ctx,
        &sender,
        EngineConfig {
            in_flight: 1,
            ..fast()
        },
    );
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!(done.progress.sent, 4);
    assert_eq!(done.notice, None);
    let sent = sender.sent();
    assert_eq!(sent.len(), 5);
    let refused = sent.iter().position(|s| s.chat == 3002).unwrap();
    let next = &sent[refused + 1];
    assert!(
        next.at.duration_since(sent[refused].at) >= Duration::from_millis(1100),
        "после 429 отправка продолжилась раньше паузы"
    );
    let mut per_chat: HashMap<i64, usize> = HashMap::new();
    for s in &sent {
        *per_chat.entry(s.chat).or_default() += 1;
    }
    assert_eq!(per_chat[&3002], 2);
    assert!(per_chat.iter().all(|(chat, n)| *chat == 3002 || *n == 1));
    let mut conn = store.conn().await.unwrap();
    let attempts: i32 = sqlx::query_scalar(
        "SELECT attempts FROM broadcast_recipients WHERE broadcast_id = $1 AND chat_id = 3002",
    )
    .bind(created.id)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(attempts, 1);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn сбой_сети_повторяется_а_потом_считается_недоставкой() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    clients(store, 3501, 2).await;
    let net = || Delivery::Failed {
        error: "Telegram недоступен: timeout".into(),
        retry: true,
    };
    sender.answer(3501, vec![net(), Delivery::Sent { file_id: None }]);
    sender.answer(3502, vec![net(), net(), net()]);
    let created = create(&ctx, &draft("Проверка", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let engine = spawn_engine(&ctx, &sender, fast());
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!((done.progress.sent, done.progress.failed), (1, 1));
    assert_eq!(done.problems[0].error, "Telegram недоступен: timeout");
    assert_eq!(sender.sent().len(), 5);
    db.cleanup().await;
}

#[tokio::test]
async fn темп_не_выше_заданного_и_чат_не_чаще_раза_в_секунду() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    clients(store, 4001, 12).await;
    let first = create(&ctx, &draft("Первая", Audience::All), None)
        .await
        .unwrap();
    let second = create(&ctx, &draft("Вторая", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, first.id, None, None).await.unwrap();
    start(&ctx, second.id, None, None).await.unwrap();
    let engine = spawn_engine(
        &ctx,
        &sender,
        EngineConfig {
            per_second: 20,
            chat_gap: Duration::from_secs(1),
            ..fast()
        },
    );
    wait_status(store, second.id, Status::Done).await;
    engine.stop().await;
    let sent = sender.sent();
    assert_eq!(sent.len(), 24);
    let mut starts: Vec<Instant> = sent.iter().map(|s| s.at).collect();
    starts.sort_unstable();
    for window in starts.windows(21) {
        assert!(
            window[20].duration_since(window[0]) >= Duration::from_millis(950),
            "больше 20 сообщений за секунду"
        );
    }
    let mut by_chat: HashMap<i64, Vec<Instant>> = HashMap::new();
    for s in &sent {
        by_chat.entry(s.chat).or_default().push(s.at);
    }
    for (chat, mut times) in by_chat {
        times.sort_unstable();
        assert_eq!(times.len(), 2);
        assert!(
            times[1].duration_since(times[0]) >= Duration::from_millis(990),
            "чат {chat} получил два сообщения быстрее секунды"
        );
    }
    db.cleanup().await;
}

async fn queue(conn: &mut sn_store::Conn) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO broadcasts (text, audience, status, started_at) \
         VALUES ('x', '{\"kind\":\"all\"}', 'sending', now()) RETURNING id",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO broadcast_recipients (broadcast_id, client_id, chat_id) \
         SELECT $1, c.id, 9000000000 + c.id FROM clients c",
    )
    .bind(id)
    .execute(conn.conn())
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn захват_берёт_ровно_столько_сколько_просили_на_большой_очереди() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO clients (registration_source) SELECT 'bot' FROM generate_series(1, 600)",
    )
    .execute(conn.conn())
    .await
    .unwrap();
    let earlier = queue(&mut conn).await;
    sqlx::query("ANALYZE broadcast_recipients")
        .execute(conn.conn())
        .await
        .unwrap();
    let id = queue(&mut conn).await;
    assert_ne!(earlier, id);
    let claimed = store::claim(&mut conn, id, Lane::Bot, 1).await.unwrap();
    assert_eq!(claimed.len(), 1);
    let flying: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM broadcast_recipients WHERE broadcast_id = $1 AND state = 'sending'",
    )
    .bind(id)
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(flying, 1);
    let more = store::claim(&mut conn, id, Lane::Bot, 3).await.unwrap();
    assert_eq!(more.len(), 3);
    assert!(more.iter().all(|c| c.client_id != claimed[0].client_id));
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn перезапуск_не_дублирует_и_не_теряет_очередь() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::slow(Duration::from_millis(20));
    let ctx = context(store, Some(sender.clone()));
    clients(store, 5001, 30).await;
    let created = create(&ctx, &draft("Большая рассылка", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let config = EngineConfig {
        per_second: 60,
        in_flight: 2,
        ..fast()
    };
    let first = spawn_engine(&ctx, &sender, config.clone());
    let started = Instant::now();
    while sender.sent().len() < 8 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    first.stop().await;
    let mut conn = store.conn().await.unwrap();
    let midway = store::view(&mut conn, created.id).await.unwrap();
    assert_eq!(midway.status, Status::Sending);
    assert!(midway.progress.pending > 0, "{midway:?}");
    let lease: Option<serde_json::Value> = conn.bot_state().get(engine::LEASE_KEY).await.unwrap();
    assert_eq!(lease, None);
    let crashed: Vec<(i64,)> = sqlx::query_as(
        "UPDATE broadcast_recipients SET state = 'sending', attempts = attempts + 1, claimed_at = now() \
         WHERE broadcast_id = $1 AND client_id IN (SELECT client_id FROM broadcast_recipients \
         WHERE broadcast_id = $1 AND state = 'pending' ORDER BY client_id LIMIT 2) RETURNING chat_id",
    )
    .bind(created.id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    drop(conn);
    let before = sender.sent().len();
    let second = spawn_engine(&ctx, &sender, config);
    let done = wait_status(store, created.id, Status::Done).await;
    second.stop().await;
    let chats = sender.chats();
    let mut unique = chats.clone();
    unique.dedup();
    assert_eq!(unique.len(), chats.len(), "кому-то ушло дважды");
    for (chat,) in &crashed {
        assert!(
            !chats.contains(chat),
            "прерванная отправка повторилась для {chat}"
        );
    }
    assert_eq!(chats.len(), 28);
    assert!(sender.sent().len() > before);
    assert_eq!(done.progress.sent, 28);
    assert_eq!(done.progress.failed, 2);
    assert!(done.problems[0].error.contains("перезапуск"));
    db.cleanup().await;
}

#[tokio::test]
async fn отмена_останавливает_оставшихся() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::slow(Duration::from_millis(30));
    let ctx = context(store, Some(sender.clone()));
    clients(store, 6001, 20).await;
    let created = create(&ctx, &draft("Отменим", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let engine = spawn_engine(
        &ctx,
        &sender,
        EngineConfig {
            per_second: 20,
            in_flight: 1,
            ..fast()
        },
    );
    let started = Instant::now();
    while sender.sent().len() < 3 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let canceled = cancel(&ctx, created.id, None).await.unwrap();
    assert_eq!(canceled.status, Status::Canceled);
    tokio::time::sleep(Duration::from_millis(300)).await;
    engine.stop().await;
    let after = sender.sent().len();
    assert!(after <= 5, "после отмены ушло ещё {after}");
    let view = view(&ctx, created.id).await.unwrap();
    assert_eq!(view.progress.pending, 0);
    assert_eq!(
        view.progress.sent + view.progress.skipped,
        20,
        "{:?}",
        view.progress
    );
    assert_eq!(
        code(cancel(&ctx, created.id, None).await.unwrap_err()),
        "broadcast.state"
    );
    let copied = copy(&ctx, created.id, None).await.unwrap();
    assert_eq!(copied.status, Status::Draft);
    assert_eq!(copied.text, "Отменим");
    db.cleanup().await;
}

#[tokio::test]
async fn запланированная_начинается_в_срок_а_отмена_возвращает_черновик() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    clients(store, 7001, 3).await;
    let created = create(&ctx, &draft("Утром", Audience::All), None)
        .await
        .unwrap();
    let now = chrono::Utc::now();
    assert_eq!(
        code(
            start(&ctx, created.id, Some(now - Span::minutes(5)), None)
                .await
                .unwrap_err()
        ),
        "broadcast.invalid"
    );
    assert_eq!(
        code(
            start(
                &ctx,
                created.id,
                Some(now + Span::days(SCHEDULE_DAYS_MAX + 1)),
                None
            )
            .await
            .unwrap_err()
        ),
        "broadcast.invalid"
    );
    let scheduled = start(&ctx, created.id, Some(now + Span::hours(2)), None)
        .await
        .unwrap();
    assert_eq!(scheduled.status, Status::Scheduled);
    assert_eq!(scheduled.progress.total, 0);
    let back = cancel(&ctx, created.id, None).await.unwrap();
    assert_eq!(back.status, Status::Draft);
    assert_eq!(back.send_at, None);
    start(&ctx, created.id, Some(now + Span::hours(1)), None)
        .await
        .unwrap();
    let engine = spawn_engine(&ctx, &sender, fast());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(sender.sent().is_empty(), "ушло раньше времени");
    exec(
        store,
        "UPDATE broadcasts SET send_at = now() - interval '1 second' WHERE id = $1",
        created.id,
    )
    .await;
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!(done.progress.sent, 3);
    let nobody = create(&ctx, &draft("Пусто", Audience::Trial), None)
        .await
        .unwrap();
    assert_eq!(
        code(start(&ctx, nobody.id, None, None).await.unwrap_err()),
        "broadcast.empty"
    );
    assert_eq!(view(&ctx, nobody.id).await.unwrap().status, Status::Draft);
    db.cleanup().await;
}

#[tokio::test]
async fn картинка_грузится_один_раз_а_дальше_идёт_по_file_id() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    clients(store, 8001, 5).await;
    let image = upload_image(&ctx, &png(800, 400), None).await.unwrap();
    let mut message = draft("С картинкой", Audience::All);
    message.image_id = Some(image.id);
    sender.answer(8003, vec![Delivery::StaleFile]);
    let created = create(&ctx, &message, None).await.unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let engine = spawn_engine(&ctx, &sender, fast());
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!(done.progress.sent, 5);
    let photos: Vec<String> = sender
        .sent()
        .into_iter()
        .map(|s| s.photo.unwrap_or_default())
        .collect();
    assert_eq!(photos[0], "upload:image/png");
    assert_eq!(photos.iter().filter(|p| p.starts_with("upload")).count(), 2);
    assert_eq!(photos.len(), 6);
    assert!(photos[1].starts_with("cached:F-1"));
    let mut conn = store.conn().await.unwrap();
    let stored = store::image(&mut conn, image.id).await.unwrap();
    assert_eq!(stored.file_for(Some(77)), Some("F-1"));
    assert_eq!(stored.file_for(Some(78)), None);
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn проверка_уходит_выбранному_клиенту() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    let client = tg_client(store, 9001).await;
    let image = upload_image(&ctx, &png(300, 300), None).await.unwrap();
    let mut message = draft("Проверка <i>оформления</i>", Audience::All);
    message.image_id = Some(image.id);
    let report = send_test(&ctx, &message, TestTarget::Client(client), None)
        .await
        .unwrap();
    assert_eq!(report.result, TestResult::Sent);
    let sent = sender.sent();
    assert_eq!(sent[0].chat, 9001);
    assert_eq!(sent[0].photo.as_deref(), Some("upload:image/png"));
    send_test(&ctx, &message, TestTarget::Client(client), None)
        .await
        .unwrap();
    assert_eq!(sender.sent()[1].photo.as_deref(), Some("cached:F-1"));
    let mail = testing::client(store).await;
    assert_eq!(
        code(
            send_test(&ctx, &message, TestTarget::Client(mail), None)
                .await
                .unwrap_err()
        ),
        "broadcast.no_telegram"
    );
    sender.answer(
        9001,
        vec![Delivery::Blocked("Клиент остановил бота".into())],
    );
    let blocked = send_test(&ctx, &message, TestTarget::Client(client), None)
        .await
        .unwrap();
    assert_eq!(blocked.result, TestResult::Blocked);
    let mut conn = store.conn().await.unwrap();
    assert!(conn.clients().get(client).await.unwrap().bot_unreachable);
    drop(conn);
    let offline = context(store, None);
    assert_eq!(
        code(
            send_test(&offline, &message, TestTarget::Client(client), None)
                .await
                .unwrap_err()
        ),
        "broadcast.bot"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn отклоняемое_телеграмом_сообщение_останавливает_рассылку() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let sender = FakeSender::new();
    let ctx = context(store, Some(sender.clone()));
    clients(store, 9101, 10).await;
    for chat in 9101..9111 {
        sender.answer(
            chat,
            vec![Delivery::Failed {
                error: "Telegram не принял адрес ссылки в кнопке".into(),
                retry: false,
            }],
        );
    }
    let created = create(&ctx, &draft("Сломанная кнопка", Audience::All), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let engine = spawn_engine(
        &ctx,
        &sender,
        EngineConfig {
            in_flight: 1,
            ..fast()
        },
    );
    let stopped = wait_status(store, created.id, Status::Canceled).await;
    engine.stop().await;
    assert!(stopped.notice.unwrap_or_default().contains("адрес ссылки"));
    assert_eq!(stopped.progress.failed, 3);
    assert_eq!(stopped.progress.skipped, 7);
    db.cleanup().await;
}

pub(super) fn admin(role: AdminRole) -> AdminPrincipal {
    AdminPrincipal {
        admin_id: sn_domain::ids::AdminId(1),
        role,
        permissions: Vec::new(),
        session_id: 1,
        api_key: None,
    }
}

pub(super) async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) async fn owner(db: &TestDb) {
    let mut conn = db.store.conn().await.unwrap();
    conn.admins()
        .create(&sn_store::NewAdmin {
            email: "owner@example.com".into(),
            password_hash: "x".into(),
            role: AdminRole::Owner,
            must_change_password: false,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn админка_собирает_считает_отправляет_и_останавливает() {
    let Some(db) = test_store().await else {
        return;
    };
    owner(&db).await;
    let sender = FakeSender::new();
    let ctx = context(&db.store, Some(sender.clone()));
    clients(&db.store, 9201, 3).await;
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx.clone());
    let call = |method: &str, uri: &str, role: AdminRole, body: Option<serde_json::Value>| {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/broadcast{uri}"))
            .extension(admin(role));
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap()
    };
    let denied = router
        .clone()
        .oneshot(call("GET", "/broadcasts", AdminRole::Support, None))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let options = router
        .clone()
        .oneshot(call("GET", "/options", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(options.status(), StatusCode::OK);
    let options = json_of(options).await;
    assert_eq!(options["bot"], true);
    assert_eq!(options["limits"]["caption"], 1024);
    assert_eq!(options["sections"][0]["key"], "menu");
    let reach = router
        .clone()
        .oneshot(call(
            "POST",
            "/audience",
            AdminRole::Admin,
            Some(serde_json::json!({ "audience": { "kind": "all" } })),
        ))
        .await
        .unwrap();
    assert_eq!(json_of(reach).await["recipients"], 3);
    let bad = router
        .clone()
        .oneshot(call(
            "POST",
            "/broadcasts",
            AdminRole::Admin,
            Some(serde_json::json!({ "text": "<b>x", "audience": { "kind": "all" } })),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_of(bad).await["error"]["code"], "broadcast.invalid");
    let created = router
        .clone()
        .oneshot(call(
            "POST",
            "/broadcasts",
            AdminRole::Admin,
            Some(serde_json::json!({
                "text": "Привет",
                "buttons": [{ "text": "Меню", "section": "menu" }],
                "audience": { "kind": "all" }
            })),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let id = json_of(created).await["id"].as_i64().unwrap();
    let image = router
        .clone()
        .oneshot(
            Request::post("/broadcast/images")
                .header("content-type", "image/gif")
                .extension(admin(AdminRole::Admin))
                .body(Body::from(b"GIF89a".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(image.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let uploaded = router
        .clone()
        .oneshot(
            Request::post("/broadcast/images")
                .header("content-type", "image/png")
                .extension(admin(AdminRole::Admin))
                .body(Body::from(png(400, 300)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::OK);
    let image_id = json_of(uploaded).await["id"].as_i64().unwrap();
    let file = router
        .clone()
        .oneshot(call(
            "GET",
            &format!("/images/{image_id}"),
            AdminRole::Admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(file.headers()["content-type"], "image/png");
    let test = router
        .clone()
        .oneshot(call(
            "POST",
            "/test",
            AdminRole::Admin,
            Some(serde_json::json!({ "client_id": 1, "text": "Проверка" })),
        ))
        .await
        .unwrap();
    assert_eq!(json_of(test).await["result"], "sent");
    let sent = router
        .clone()
        .oneshot(call(
            "POST",
            &format!("/broadcasts/{id}/send"),
            AdminRole::Admin,
            Some(serde_json::json!({ "at": null })),
        ))
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::OK);
    let sent = json_of(sent).await;
    assert_eq!(sent["status"], "sending");
    assert_eq!(sent["progress"]["total"], 3);
    let stopped = router
        .clone()
        .oneshot(call(
            "POST",
            &format!("/broadcasts/{id}/cancel"),
            AdminRole::Admin,
            None,
        ))
        .await
        .unwrap();
    let stopped = json_of(stopped).await;
    assert_eq!(stopped["status"], "canceled");
    assert_eq!(stopped["progress"]["skipped"], 3);
    let listed = router
        .clone()
        .oneshot(call("GET", "/broadcasts?limit=10", AdminRole::Admin, None))
        .await
        .unwrap();
    let listed = json_of(listed).await;
    assert_eq!(listed["items"][0]["id"], id);
    assert_eq!(listed["items"][0]["status"], "canceled");
    let missing = router
        .oneshot(call("GET", "/broadcasts/999999", AdminRole::Admin, None))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let mut conn = db.store.conn().await.unwrap();
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN ('broadcast.started', 'broadcast.canceled', 'broadcast.test')",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(audit, 3);
    drop(conn);
    db.cleanup().await;
}

pub(super) async fn mail_client(store: &Store, chat: Option<i64>, email: Option<&str>) -> ClientId {
    let id = testing::client(store).await;
    let mut conn = store.conn().await.unwrap();
    if let Some(chat) = chat {
        conn.clients()
            .add_identity(
                id,
                &NewIdentity {
                    kind: IdentityKind::Telegram,
                    value: chat.to_string(),
                    verified: true,
                    meta: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
    }
    if let Some(email) = email {
        conn.clients()
            .add_identity(
                id,
                &NewIdentity {
                    kind: IdentityKind::Email,
                    value: email.to_string(),
                    verified: false,
                    meta: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
    }
    id
}

pub(super) fn fast_mail() -> EngineConfig {
    EngineConfig {
        per_second: 200,
        chat_gap: Duration::ZERO,
        in_flight: 2,
        idle: Duration::from_millis(40),
        retries: 3,
        retry_pause: Duration::from_millis(40),
        bot_pause: Duration::from_millis(200),
        lease: Duration::from_secs(2),
        grace: Duration::from_secs(2),
        reject_limit: 3,
    }
}

pub(super) fn spawn_mail(
    ctx: &ModuleContext,
    mail: &Arc<FakeMail>,
    config: EngineConfig,
) -> Running {
    let (stop, stopped) = oneshot::channel::<()>();
    let engine = Engine::new(ctx.clone(), Sender::Mail(mail.clone()), || true, config);
    let handle = tokio::spawn(engine.run(async move {
        let _ = stopped.await;
    }));
    Running { stop, handle }
}

fn mail_draft(text: &str, channel: Channel) -> Draft {
    Draft {
        channel,
        subject: Some("Новости сервиса".into()),
        ..draft(text, Audience::All)
    }
}

async fn tokens(store: &Store, id: i64) -> HashMap<String, Uuid> {
    let mut conn = store.conn().await.unwrap();
    let rows: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT email, unsubscribe FROM broadcast_recipients WHERE broadcast_id = $1 AND channel = 'email'",
    )
    .bind(id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    rows.into_iter().collect()
}

#[test]
fn тема_письма_нужна_для_почты() {
    assert_eq!(clean_subject(None, Channel::Bot).unwrap(), None);
    assert_eq!(
        clean_subject(Some("  Новые   серверы "), Channel::Email)
            .unwrap()
            .as_deref(),
        Some("Новые серверы")
    );
    assert!(clean_subject(Some("   "), Channel::Both).is_err());
    assert!(clean_subject(None, Channel::Email).is_err());
    assert!(clean_subject(Some("Тема\u{7}"), Channel::Email).is_err());
    assert!(clean_subject(Some(&"т".repeat(SUBJECT_MAX + 1)), Channel::Email).is_err());
    assert_eq!(Channel::Both.lanes(), vec![Lane::Bot, Lane::Email]);
    assert!(Channel::Email.has(Lane::Email) && !Channel::Email.has(Lane::Bot));
    assert_eq!(BotSection::Balance.web_path(), "/cabinet/topup");
    assert_eq!(BotSection::Buy.web_path(), "/cabinet/pay");
    assert_eq!(
        serde_json::from_value::<Draft>(serde_json::json!({ "audience": { "kind": "all" } }))
            .unwrap()
            .channel,
        Channel::Bot
    );
}

#[test]
fn письмо_собирается_из_того_же_текста() {
    let site = MailSite {
        base_url: "https://vpn.example.com".into(),
        brand: "Тест & Ко".into(),
    };
    let text = clean_text(
        "<b>Новые серверы</b>\nСкорость <a href=\"https://example.com/news?a=1&b=2\">выше</a> \
         <tg-emoji emoji-id=\"5368324170671202286\">🔥</tg-emoji>\n<span class=\"tg-spoiler\">секрет</span> &amp; <code>код</code>",
        false,
    )
    .unwrap();
    let buttons = clean_buttons(&[
        ButtonSpec {
            text: "Продлить".into(),
            url: None,
            section: Some(BotSection::Buy),
        },
        ButtonSpec {
            text: "Канал".into(),
            url: Some("t.me/news".into()),
            section: None,
        },
    ])
    .unwrap();
    let letter = letter_for("Новые серверы", &text, Some(Uuid::nil()), &buttons, &site);
    let html = letter.html("https://vpn.example.com/u/1");
    assert!(html.contains("<b>Новые серверы</b><br>Скорость"), "{html}");
    assert!(html.contains("href=\"https://example.com/news?a=1&amp;b=2\""));
    assert!(html.contains("🔥") && !html.contains("tg-emoji"));
    assert!(html.contains("<span>секрет</span>"));
    assert!(html.contains(
        "src=\"https://vpn.example.com/api/public/m/broadcast/images/00000000-0000-0000-0000-000000000000\""
    ));
    assert!(html.contains("href=\"https://vpn.example.com/cabinet/pay\""));
    assert!(html.contains("href=\"https://t.me/news\""));
    assert!(html.contains("аккаунт в Тест &amp; Ко"));
    assert!(html.contains("href=\"https://vpn.example.com/u/1\""));
    assert!(html.contains("<title>Новые серверы</title>"));
    let plain = letter.text("https://vpn.example.com/u/1");
    assert!(
        plain.starts_with(
            "Новые серверы\nСкорость выше (https://example.com/news?a=1&b=2) 🔥\nсекрет & код"
        ),
        "{plain}"
    );
    assert!(plain.contains("\nПродлить: https://vpn.example.com/cabinet/pay"));
    assert!(plain.contains("\nКанал: https://t.me/news"));
    assert!(plain.ends_with("Отписаться от рассылок: https://vpn.example.com/u/1\n"));
}

#[tokio::test]
async fn на_почту_привязанную_без_кода_рассылка_не_уходит() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    let client = mail_client(store, Some(11101), None).await;
    {
        let mut conn = store.conn().await.unwrap();
        conn.clients()
            .add_identity(
                client,
                &NewIdentity::email_without_code("raw@example.com").unwrap(),
            )
            .await
            .unwrap();
    }
    let r = reach(&ctx, Audience::All).await.unwrap();
    assert_eq!((r.recipients, r.emails), (1, 0));
    exec(
        store,
        "UPDATE client_identities SET verified = true WHERE kind = 'email' AND client_id = $1",
        client.0,
    )
    .await;
    let r = reach(&ctx, Audience::All).await.unwrap();
    assert_eq!((r.recipients, r.emails), (1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn почта_считает_адреса_и_отписки_а_оба_канала_дают_обе_строки() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    mail_client(store, Some(11001), Some("both@example.com")).await;
    mail_client(store, None, Some("mail@example.com")).await;
    mail_client(store, Some(11003), None).await;
    let gone = mail_client(store, None, Some("gone@example.com")).await;
    let mut conn = store.conn().await.unwrap();
    sqlx::query(
        "INSERT INTO broadcast_unsubscribes (email, client_id) VALUES ('gone@example.com', $1)",
    )
    .bind(gone.0)
    .execute(conn.conn())
    .await
    .unwrap();
    drop(conn);
    let r = reach(&ctx, Audience::All).await.unwrap();
    assert_eq!(
        (r.recipients, r.stopped, r.emails, r.unsubscribed),
        (2, 0, 2, 1)
    );
    let mut nosubject = mail_draft("Привет", Channel::Email);
    nosubject.subject = None;
    assert_eq!(
        code(create(&ctx, &nosubject, None).await.unwrap_err()),
        "broadcast.invalid"
    );
    let created = create(&ctx, &mail_draft("Привет", Channel::Both), None)
        .await
        .unwrap();
    assert_eq!(created.channel, Channel::Both);
    assert_eq!(created.subject.as_deref(), Some("Новости сервиса"));
    *mail.ready.lock().unwrap() = Err("модуль «Почта» выключен".into());
    let off = start(&ctx, created.id, None, None).await.unwrap_err();
    assert_eq!(code(off), "broadcast.mail");
    let options = options(&ctx).await.unwrap();
    assert!(!options.mail.ready);
    assert!(options.mail.reason.unwrap().contains("выключен"));
    assert!(!options.mail.site);
    *mail.ready.lock().unwrap() = Err(crate::sender::NO_SITE_ADDRESS.into());
    let no_site = super::options(&ctx).await.unwrap();
    assert!(!no_site.mail.ready);
    assert!(no_site.mail.site);
    assert!(no_site.mail.reason.unwrap().contains("stealthnet domain"));
    *mail.ready.lock().unwrap() = Ok(MailSite {
        base_url: "https://vpn.example.com".into(),
        brand: "Тест".into(),
    });
    let started = start(&ctx, created.id, None, None).await.unwrap();
    assert_eq!(started.progress.total, 4);
    let lanes: Vec<(Lane, i64)> = started
        .channels
        .iter()
        .map(|c| (c.channel, c.progress.total))
        .collect();
    assert_eq!(lanes, [(Lane::Bot, 2), (Lane::Email, 2)]);
    let tokens = tokens(store, created.id).await;
    assert_eq!(tokens.len(), 2);
    assert!(tokens.contains_key("both@example.com") && tokens.contains_key("mail@example.com"));
    db.cleanup().await;
}

#[tokio::test]
async fn письма_уходят_с_темпом_отпиской_и_ошибками_по_адресам() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    for i in 1..=5 {
        mail_client(store, None, Some(&format!("u{i}@example.com"))).await;
    }
    for refused in ["u2@example.com", "u5@example.com"] {
        mail.answer(
            refused,
            vec![MailDelivery::Rejected(format!(
                "Почтовый сервер отказал, код 550: 5.1.1 <{refused}>: нет такого ящика"
            ))],
        );
    }
    mail.answer(
        "u3@example.com",
        vec![
            MailDelivery::Failed {
                error: "Почтовый сервер попросил повторить позже".into(),
                retry: true,
            },
            MailDelivery::Sent,
        ],
    );
    mail.answer(
        "u4@example.com",
        vec![MailDelivery::Wait(Duration::from_millis(700))],
    );
    let mut message = mail_draft("<b>Скидка</b> на год", Channel::Email);
    message.buttons = vec![ButtonSpec {
        text: "Продлить".into(),
        url: None,
        section: Some(BotSection::Buy),
    }];
    let created = create(&ctx, &message, None).await.unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let tokens = tokens(store, created.id).await;
    let engine = spawn_mail(
        &ctx,
        &mail,
        EngineConfig {
            per_second: 5,
            in_flight: 1,
            ..fast_mail()
        },
    );
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!((done.progress.sent, done.progress.failed), (3, 2));
    assert_eq!(done.channels.len(), 1);
    assert_eq!(done.channels[0].channel, Lane::Email);
    assert_eq!(
        done.problems,
        [Problem {
            channel: Lane::Email,
            error: "Почтовый сервер отказал, код 550: 5.1.1 <адрес>: нет такого ящика".into(),
            count: 2,
        }]
    );
    assert_eq!(
        done.failed_emails,
        ["u2@example.com", "u5@example.com"]
            .map(|email| FailedEmail {
                email: email.into(),
                error: format!(
                    "Почтовый сервер отказал, код 550: 5.1.1 <{email}>: нет такого ящика"
                ),
            })
            .to_vec()
    );
    assert_eq!(done.mail_notice, None);
    let mailed = mail.mailed();
    assert_eq!(mailed.len(), 7);
    for letter in &mailed {
        assert_eq!(letter.subject, "Новости сервиса");
        let token = tokens[&letter.to];
        let url = format!("https://vpn.example.com/api/public/m/broadcast/unsubscribe/{token}");
        assert_eq!(letter.unsubscribe.as_deref(), Some(url.as_str()));
        assert!(letter.html.contains(&url) && letter.text.contains(&url));
        assert!(letter.html.contains("<b>Скидка</b> на год"));
        assert!(letter.html.contains("https://vpn.example.com/cabinet/pay"));
    }
    for pair in mailed.windows(2) {
        assert!(
            pair[1].at.duration_since(pair[0].at) >= Duration::from_millis(180),
            "письма чаще пяти в секунду"
        );
    }
    let waited = mailed
        .iter()
        .position(|m| m.to == "u4@example.com")
        .unwrap();
    assert!(mailed[waited + 1].at.duration_since(mailed[waited].at) >= Duration::from_millis(650));
    db.cleanup().await;
}

async fn wait_notice(store: &Store, id: i64) -> Broadcast {
    let started = Instant::now();
    loop {
        let mut conn = store.conn().await.unwrap();
        let view = store::view(&mut conn, id).await.unwrap();
        drop(conn);
        if view.mail_notice.is_some() {
            return view;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "{view:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn сломанная_почта_держит_письма_в_очереди_и_потом_отправляет() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    for i in 1..=3 {
        mail_client(store, None, Some(&format!("w{i}@example.com"))).await;
    }
    *mail.fallback.lock().unwrap() = Some(MailDelivery::Off(
        "почтовый сервер не принял логин и пароль (535)".into(),
    ));
    let created = create(&ctx, &mail_draft("Ждём почту", Channel::Email), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let engine = spawn_mail(&ctx, &mail, fast_mail());
    let waiting = wait_notice(store, created.id).await;
    assert!(waiting.mail_notice.unwrap().contains("535"));
    assert_eq!(waiting.notice, None);
    assert_eq!(waiting.progress.sent, 0);
    assert_eq!(waiting.status, Status::Sending);
    *mail.fallback.lock().unwrap() = None;
    let done = wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    assert_eq!(done.progress.sent, 3);
    assert_eq!(done.mail_notice, None);
    let mut conn = store.conn().await.unwrap();
    let attempts: Vec<i32> = sqlx::query_scalar(
        "SELECT attempts FROM broadcast_recipients WHERE broadcast_id = $1 ORDER BY client_id",
    )
    .bind(created.id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert!(attempts.iter().all(|a| *a == 1), "{attempts:?}");
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn в_бот_и_на_почту_уходит_одновременно() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let bot = FakeSender::new();
    let mail = FakeMail::new();
    let ctx = context_with(store, Some(bot.clone()), Some(mail.clone()));
    mail_client(store, Some(12001), Some("a@example.com")).await;
    mail_client(store, Some(12002), None).await;
    mail_client(store, None, Some("c@example.com")).await;
    let created = create(&ctx, &mail_draft("Всем", Channel::Both), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let bots = spawn_engine(&ctx, &bot, fast());
    let mails = spawn_mail(&ctx, &mail, fast_mail());
    let done = wait_status(store, created.id, Status::Done).await;
    bots.stop().await;
    mails.stop().await;
    assert_eq!(done.progress.sent, 4);
    let lanes: Vec<(Lane, i64)> = done
        .channels
        .iter()
        .map(|c| (c.channel, c.progress.sent))
        .collect();
    assert_eq!(lanes, [(Lane::Bot, 2), (Lane::Email, 2)]);
    assert_eq!(bot.chats(), [12001, 12002]);
    let mut to: Vec<String> = mail.mailed().into_iter().map(|m| m.to).collect();
    to.sort();
    assert_eq!(to, ["a@example.com", "c@example.com"]);
    let mut conn = store.conn().await.unwrap();
    let leases: Vec<String> =
        sqlx::query_scalar("SELECT key FROM bot_state WHERE key LIKE 'broadcast.%' ORDER BY key")
            .fetch_all(conn.conn())
            .await
            .unwrap();
    assert!(leases.is_empty(), "{leases:?}");
    drop(conn);
    db.cleanup().await;
}

async fn text_of(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn отписка_по_ссылке_из_письма_и_картинка_для_писем() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    mail_client(store, None, Some("reader@example.com")).await;
    mail_client(store, None, Some("other@example.com")).await;
    let image = upload_image(&ctx, &png(640, 360), None).await.unwrap();
    let mut message = mail_draft("С картинкой", Channel::Email);
    message.image_id = Some(image.id);
    let created = create(&ctx, &message, None).await.unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let token = tokens(store, created.id).await["reader@example.com"];
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.public_router(ctx.clone());
    let get = |uri: String| Request::get(uri).body(Body::empty()).unwrap();
    let post = |uri: String| {
        Request::post(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("List-Unsubscribe=One-Click"))
            .unwrap()
    };
    let page = router
        .clone()
        .oneshot(get(format!("/broadcast/unsubscribe/{token}")))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        page.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("default-src 'none'")
    );
    let page = text_of(page).await;
    assert!(page.contains("Отписаться от рассылок?") && page.contains("reader@example.com"));
    assert_eq!(reach(&ctx, Audience::All).await.unwrap().unsubscribed, 0);
    let done = router
        .clone()
        .oneshot(post(format!("/broadcast/unsubscribe/{token}")))
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::OK);
    assert!(text_of(done).await.contains("Вы отписаны"));
    let again = router
        .clone()
        .oneshot(post(format!("/broadcast/unsubscribe/{token}")))
        .await
        .unwrap();
    assert!(text_of(again).await.contains("Вы отписаны"));
    let counted = reach(&ctx, Audience::All).await.unwrap();
    assert_eq!((counted.emails, counted.unsubscribed), (1, 1));
    let back = router
        .clone()
        .oneshot(post(format!("/broadcast/resubscribe/{token}")))
        .await
        .unwrap();
    assert!(text_of(back).await.contains("Рассылки вернутся"));
    assert_eq!(reach(&ctx, Audience::All).await.unwrap().unsubscribed, 0);
    let unknown = router
        .clone()
        .oneshot(get(format!("/broadcast/unsubscribe/{}", Uuid::new_v4())))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let preview = router
        .clone()
        .oneshot(post(format!("/broadcast/unsubscribe/{}", Uuid::nil())))
        .await
        .unwrap();
    assert!(text_of(preview).await.contains("проверочное"));
    let broken = router
        .clone()
        .oneshot(get("/broadcast/unsubscribe/abc".into()))
        .await
        .unwrap();
    assert_eq!(broken.status(), StatusCode::BAD_REQUEST);
    let mut conn = store.conn().await.unwrap();
    let key = store::image_key(&mut conn, image.id).await.unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN ('broadcast.unsubscribed', 'broadcast.resubscribed')",
    )
    .fetch_one(conn.conn())
    .await
    .unwrap();
    assert_eq!(audited, 2);
    drop(conn);
    let picture = router
        .clone()
        .oneshot(get(format!("/broadcast/images/{key}")))
        .await
        .unwrap();
    assert_eq!(picture.status(), StatusCode::OK);
    assert_eq!(picture.headers()["content-type"], "image/png");
    assert!(
        picture.headers()["cache-control"]
            .to_str()
            .unwrap()
            .starts_with("public")
    );
    let missing = router
        .clone()
        .oneshot(get(format!("/broadcast/images/{}", Uuid::new_v4())))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let mut conn = db.store.conn().await.unwrap();
    sqlx::query("UPDATE broadcast_images SET bytes = '\\x00' WHERE public_key = $1")
        .bind(key)
        .execute(conn.conn())
        .await
        .unwrap();
    drop(conn);
    let again = router
        .oneshot(get(format!("/broadcast/images/{key}")))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    let body = axum::body::to_bytes(again.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        body.len() > 1,
        "картинка письма берётся из памяти, а не из базы"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn проверка_письмом_уходит_на_указанный_адрес() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    owner(&db).await;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    let mut message = mail_draft("Проверка <i>письма</i>", Channel::Bot);
    message.subject = Some("Тема проверки".into());
    let report = send_test(
        &ctx,
        &message,
        TestTarget::Address(" Owner@Example.com ".into()),
        None,
    )
    .await
    .unwrap();
    assert_eq!(report.result, TestResult::Sent);
    assert!(report.message.contains("owner@example.com"));
    let sent = &mail.mailed()[0];
    assert_eq!(sent.to, "owner@example.com");
    assert_eq!(sent.subject, "Тема проверки");
    assert!(
        sent.unsubscribe
            .as_deref()
            .unwrap()
            .ends_with(&Uuid::nil().to_string())
    );
    assert!(sent.html.contains("<i>письма</i>"));
    assert_eq!(
        code(
            send_test(
                &ctx,
                &message,
                TestTarget::Address("нет адреса".into()),
                None
            )
            .await
            .unwrap_err()
        ),
        "broadcast.address"
    );
    let mut nosubject = message.clone();
    nosubject.subject = None;
    assert_eq!(
        code(
            send_test(
                &ctx,
                &nosubject,
                TestTarget::Address("a@example.com".into()),
                None
            )
            .await
            .unwrap_err()
        ),
        "broadcast.invalid"
    );
    mail.answer(
        "bad@example.com",
        vec![MailDelivery::Rejected("Почтовый сервер отказал".into())],
    );
    assert_eq!(
        code(
            send_test(
                &ctx,
                &message,
                TestTarget::Address("bad@example.com".into()),
                None
            )
            .await
            .unwrap_err()
        ),
        "broadcast.mail"
    );
    *mail.ready.lock().unwrap() = Err("в «Почте» не выбран способ отправки писем".into());
    assert_eq!(
        code(
            send_test(
                &ctx,
                &message,
                TestTarget::Address("a@example.com".into()),
                None
            )
            .await
            .unwrap_err()
        ),
        "broadcast.mail"
    );
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    *mail.ready.lock().unwrap() = Ok(MailSite {
        base_url: "https://vpn.example.com".into(),
        brand: "Тест".into(),
    });
    let routed = registry
        .admin_router(ctx.clone())
        .oneshot(
            Request::post("/broadcast/test")
                .header("content-type", "application/json")
                .extension(admin(AdminRole::Admin))
                .body(Body::from(
                    serde_json::json!({
                        "email": "route@example.com",
                        "subject": "Через маршрут",
                        "text": "Привет"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(routed.status(), StatusCode::OK);
    assert_eq!(json_of(routed).await["result"], "sent");
    assert_eq!(mail.mailed().last().unwrap().to, "route@example.com");
    db.cleanup().await;
}

#[tokio::test]
async fn подвал_письма_и_страница_отписки_на_языке_получателя() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, None, Some(mail.clone()));
    let ann = mail_client(store, None, Some("ann@example.com")).await;
    mail_client(store, None, Some("ivan@example.com")).await;
    exec(
        store,
        "UPDATE clients SET locale = 'en' WHERE id = $1",
        ann.0,
    )
    .await;
    {
        let mut conn = store.conn().await.unwrap();
        conn.settings()
            .set_as(
                "core.languages",
                &serde_json::json!({ "english": true, "default": "ru" }),
                None,
            )
            .await
            .unwrap();
    }
    let created = create(&ctx, &mail_draft("<b>Новости</b>", Channel::Email), None)
        .await
        .unwrap();
    start(&ctx, created.id, None, None).await.unwrap();
    let tokens = tokens(store, created.id).await;
    let engine = spawn_mail(&ctx, &mail, fast_mail());
    wait_status(store, created.id, Status::Done).await;
    engine.stop().await;
    let mailed = mail.mailed();
    let english = mailed.iter().find(|m| m.to == "ann@example.com").unwrap();
    let russian = mailed.iter().find(|m| m.to == "ivan@example.com").unwrap();
    assert!(english.html.contains("Unsubscribe") && english.html.contains("lang=\"en\""));
    assert!(english.text.contains("You're getting this email because"));
    assert!(
        russian.html.contains("Отписаться от рассылок") && russian.html.contains("lang=\"ru\"")
    );
    let router =
        Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap()).public_router(ctx.clone());
    let page = |token: Uuid| {
        router.clone().oneshot(
            Request::get(format!("/broadcast/unsubscribe/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    let shown = text_of(page(tokens["ann@example.com"]).await.unwrap()).await;
    assert!(shown.contains("Unsubscribe from newsletters?"), "{shown}");
    let shown = text_of(page(tokens["ivan@example.com"]).await.unwrap()).await;
    assert!(shown.contains("Отписаться от рассылок?"), "{shown}");
    exec(
        store,
        "DELETE FROM settings WHERE key = 'core.languages' AND $1 > 0",
        1,
    )
    .await;
    let shown = text_of(page(tokens["ann@example.com"]).await.unwrap()).await;
    assert!(shown.contains("Отписаться от рассылок?"), "{shown}");
    db.cleanup().await;
}

#[test]
fn аудитория_по_условиям_пишется_читается_и_описывается() {
    let raw = serde_json::json!({
        "kind": "filter",
        "tariff": [5],
        "status": "active",
        "source": ["bot"],
    });
    let audience: Audience = serde_json::from_value(raw.clone()).unwrap();
    let Audience::Filter(conditions) = &audience else {
        panic!("ожидались условия: {audience:?}");
    };
    assert_eq!(conditions.tariff, vec![TariffId(5)]);
    assert_eq!(serde_json::to_value(&audience).unwrap(), raw);
    let everyone: Audience =
        serde_json::from_value(serde_json::json!({ "kind": "filter" })).unwrap();
    assert_eq!(everyone, Audience::Filter(ClientConditions::default()));
    assert_eq!(everyone.describe(None, None), "Все клиенты");
    let names = AudienceNames {
        tariffs: HashMap::from([(TariffId(5), "Про".to_string())]),
        categories: HashMap::new(),
        currency: Currency::Rub,
    };
    assert_eq!(
        audience.describe(None, Some(&names)),
        "Тариф «Про», подписка действует, откуда: бот"
    );
    assert!(!audience.uses_money());
    let money: Audience =
        serde_json::from_value(serde_json::json!({ "kind": "filter", "paid": true })).unwrap();
    assert!(money.uses_money());
    let blocked: Audience =
        serde_json::from_value(serde_json::json!({ "kind": "filter", "status": "blocked" }))
            .unwrap();
    assert_eq!(code(blocked.check().unwrap_err()), "broadcast.invalid");
    let zero: Audience =
        serde_json::from_value(serde_json::json!({ "kind": "filter", "ends": 0 })).unwrap();
    assert_eq!(code(zero.check().unwrap_err()), "broadcast.invalid");
    assert!(
        serde_json::from_value::<Audience>(
            serde_json::json!({ "kind": "filter", "status": "lost" })
        )
        .is_err()
    );
    let legacy: Audience =
        serde_json::from_value(serde_json::json!({ "kind": "tariff", "tariff_id": 5 })).unwrap();
    assert_eq!(legacy.describe(Some("Про"), Some(&names)), "Тариф «Про»");
    assert!(!legacy.uses_money());
}

#[tokio::test]
async fn охват_и_запуск_по_условиям() {
    let Some(db) = test_store().await else {
        return;
    };
    let store = &db.store;
    let mail = FakeMail::new();
    let ctx = context_with(store, Some(FakeSender::new()), Some(mail));
    let pro = tariff(store, "Про").await;
    let other = tariff(store, "Другой").await;
    let first = tg_client(store, 3001).await;
    subscribe(
        store,
        first,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    let stopped = tg_client(store, 3002).await;
    subscribe(
        store,
        stopped,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    exec(
        store,
        "UPDATE clients SET bot_unreachable = true WHERE id = $1",
        stopped.0,
    )
    .await;
    let blocked = tg_client(store, 3003).await;
    subscribe(
        store,
        blocked,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    exec(
        store,
        "UPDATE clients SET status = 'blocked' WHERE id = $1",
        blocked.0,
    )
    .await;
    let deleted = tg_client(store, 3004).await;
    subscribe(
        store,
        deleted,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    exec(
        store,
        "UPDATE clients SET deleted_at = now() WHERE id = $1",
        deleted.0,
    )
    .await;
    let elsewhere = tg_client(store, 3005).await;
    subscribe(
        store,
        elsewhere,
        Sub {
            tariff: Some(other),
            ..PAID
        },
    )
    .await;
    let rich = tg_client(store, 3006).await;
    subscribe(
        store,
        rich,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    exec(
        store,
        "WITH o AS (INSERT INTO orders (client_id, kind, intent, quote, currency, total_minor) \
         VALUES ($1, 'topup', '{}', '{}', 'USD', 2000) RETURNING id, client_id) \
         INSERT INTO payments (order_id, client_id, provider, amount_minor, currency, status, paid_at) \
         SELECT id, client_id, 'cryptopay', 2000, 'USD', 'paid', now() FROM o",
        rich.0,
    )
    .await;
    let letter = mail_client(store, None, Some("letter@example.com")).await;
    subscribe(
        store,
        letter,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    let gone = mail_client(store, None, Some("gone@example.com")).await;
    subscribe(
        store,
        gone,
        Sub {
            tariff: Some(pro),
            ..PAID
        },
    )
    .await;
    exec(
        store,
        "INSERT INTO broadcast_unsubscribes (email, client_id) VALUES ('gone@example.com', $1)",
        gone.0,
    )
    .await;

    let on_pro = ClientConditions {
        tariff: vec![pro],
        ..ClientConditions::default()
    };
    let found = reach(&ctx, Audience::Filter(on_pro.clone())).await.unwrap();
    assert_eq!(
        (
            found.recipients,
            found.stopped,
            found.emails,
            found.unsubscribed
        ),
        (2, 1, 1, 1),
        "заблокированные и удалённые не входят, остановившие и отписавшиеся считаются отдельно"
    );
    let paying = reach(
        &ctx,
        Audience::Filter(ClientConditions {
            paid_from: Some(10),
            ..on_pro.clone()
        }),
    )
    .await
    .unwrap();
    assert_eq!((paying.recipients, paying.emails), (1, 0));
    let stopped_only = reach(
        &ctx,
        Audience::Filter(ClientConditions {
            bot_stopped: Some(true),
            ..ClientConditions::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!((stopped_only.recipients, stopped_only.stopped), (0, 1));
    assert_eq!(
        reach(&ctx, Audience::Filter(ClientConditions::default()))
            .await
            .unwrap(),
        reach(&ctx, Audience::All).await.unwrap(),
        "без условий — как «все клиенты»"
    );
    assert_eq!(
        code(
            reach(
                &ctx,
                Audience::Filter(ClientConditions {
                    status: Some(sn_store::ClientSegment::Blocked),
                    ..ClientConditions::default()
                })
            )
            .await
            .unwrap_err()
        ),
        "broadcast.invalid"
    );

    let created = create(
        &ctx,
        &Draft {
            channel: Channel::Both,
            subject: Some("Новости".into()),
            ..draft("Привет", Audience::Filter(on_pro.clone()))
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(created.audience_label, "Тариф «Про»");
    let listed = list(&ctx, PageRequest::default()).await.unwrap();
    assert_eq!(listed.items[0].audience_label, "Тариф «Про»");
    let started = start(&ctx, created.id, None, None).await.unwrap();
    assert_eq!(started.status, Status::Sending);
    let mut conn = store.conn().await.unwrap();
    let rows: Vec<(String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT channel, chat_id, email FROM broadcast_recipients WHERE broadcast_id = $1 \
         ORDER BY channel, chat_id, email",
    )
    .bind(created.id)
    .fetch_all(conn.conn())
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("bot".to_string(), Some(3001), None),
            ("bot".to_string(), Some(3006), None),
            (
                "email".to_string(),
                None,
                Some("letter@example.com".to_string())
            ),
        ]
    );
    drop(conn);
    db.cleanup().await;
}

#[tokio::test]
async fn условия_про_деньги_только_с_правом_деньги() {
    let Some(db) = test_store().await else {
        return;
    };
    owner(&db).await;
    let ctx = context(&db.store, Some(FakeSender::new()));
    clients(&db.store, 9301, 2).await;
    let registry = Arc::new(ModuleRegistry::new(crate::catalog::all()).unwrap());
    let router = registry.admin_router(ctx);
    let marketer = AdminPrincipal {
        permissions: vec![sn_domain::access::Permission::Marketing],
        ..admin(AdminRole::Support)
    };
    let call = |method: &str, uri: &str, who: AdminPrincipal, body: Option<serde_json::Value>| {
        let mut request = Request::builder()
            .method(method)
            .uri(format!("/broadcast{uri}"))
            .extension(who);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap()
    };
    let paid = serde_json::json!({ "audience": { "kind": "filter", "paid": true } });
    let denied = router
        .clone()
        .oneshot(call(
            "POST",
            "/audience",
            marketer.clone(),
            Some(paid.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_of(denied).await["error"]["code"], "auth.forbidden");
    let plain = router
        .clone()
        .oneshot(call(
            "POST",
            "/audience",
            marketer.clone(),
            Some(serde_json::json!({ "audience": { "kind": "filter", "telegram": true } })),
        ))
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    assert_eq!(json_of(plain).await["recipients"], 2);
    let draft_denied = router
        .clone()
        .oneshot(call(
            "POST",
            "/broadcasts",
            marketer.clone(),
            Some(serde_json::json!({
                "text": "Привет",
                "audience": { "kind": "filter", "balance": true },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(draft_denied.status(), StatusCode::FORBIDDEN);
    let allowed = router
        .clone()
        .oneshot(call(
            "POST",
            "/audience",
            admin(AdminRole::Admin),
            Some(paid),
        ))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(json_of(allowed).await["recipients"], 0);
    let blocked = router
        .clone()
        .oneshot(call(
            "POST",
            "/audience",
            admin(AdminRole::Admin),
            Some(serde_json::json!({ "audience": { "kind": "filter", "status": "blocked" } })),
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_of(blocked).await["error"]["code"], "broadcast.invalid");
    let options = router
        .oneshot(call("GET", "/options", admin(AdminRole::Admin), None))
        .await
        .unwrap();
    assert_eq!(options.status(), StatusCode::OK);
    let options = json_of(options).await;
    assert!(options["filter"]["tariffs"].is_array());
    assert!(options["filter"]["categories"].is_array());
    assert!(options["filter"]["utm"].is_array());
    db.cleanup().await;
}
