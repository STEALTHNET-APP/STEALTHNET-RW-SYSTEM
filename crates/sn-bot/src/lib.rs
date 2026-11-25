mod account;
pub mod alerts;
pub mod broadcast;
mod buy;
pub mod consent;
pub mod contests;
mod email;
mod extras;
mod gifts;
pub mod gram_wallet;
pub mod handler;
pub mod lanes;
pub mod link;
pub mod login;
pub mod media;
pub mod notify;
pub mod pacer;
pub mod photos;
pub mod session;
pub mod stars;
pub mod support_files;
pub mod telegram;
pub mod text;
pub mod ui;
pub mod webhook;

#[cfg(test)]
mod tests;

use handler::BotCtx;
use lanes::Lanes;
use sn_domain::settings::keys;
use sn_modules::ModuleError;
use sn_services::bot_flood::{Kind, Probe, Refusal, Verdict};
use sn_services::{BotProblem, BotState, ServiceError, Services};
use sn_store::{Db, StoreError};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use handler::handle_update;
pub use notify::NotifyHandler;
pub use pacer::SendLimits;
pub use telegram::{BotUser, TelegramApi, TelegramError, Update};

pub const OFFSET_KEY: &str = "telegram.offset";
pub const DEFAULT_CONCURRENCY: usize = 64;

const SAVE_RETRY: Duration = Duration::from_secs(1);
const SYNC_RETRY: Duration = Duration::from_secs(60);
const IDLE_RECHECK: Duration = Duration::from_secs(30);
const RETRY_PAUSE: Duration = Duration::from_secs(30);
const WEBHOOK_PAUSE: Duration = Duration::from_secs(120);
const BAD_TOKEN_PAUSE: Duration = Duration::from_secs(600);
const UNREACHABLE_AFTER: u32 = 2;
const LEASE_CHECK: Duration = Duration::from_secs(30);
const PURGE_EVERY: Duration = Duration::from_secs(600);
const FIRST_CHECK: Duration = Duration::from_secs(20);
const SIGNUPS_REPORT_EVERY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct BotConfig {
    pub api_base: String,
    pub poll_timeout: Duration,
    pub workers: usize,
    pub stars_retry_every: Duration,
    pub stars_reconcile_every: Duration,
    pub alerts_tick: Duration,
    pub alerts_every: Duration,
    pub webhook_check_every: Duration,
    pub webhook_alert_after: Duration,
    pub webhook_fallback_after: Duration,
    pub queue_recheck: Duration,
    pub send: SendLimits,
    pub queue_high: usize,
    pub queue_low: usize,
    pub chat_queue: usize,
    pub flood_report_every: Duration,
    pub offset_every: Duration,
}

impl Default for BotConfig {
    fn default() -> Self {
        BotConfig {
            api_base: telegram::DEFAULT_API.into(),
            poll_timeout: Duration::from_secs(25),
            workers: DEFAULT_CONCURRENCY,
            stars_retry_every: Duration::from_secs(30),
            stars_reconcile_every: Duration::from_secs(60),
            alerts_tick: Duration::from_secs(5),
            alerts_every: Duration::from_secs(60),
            webhook_check_every: Duration::from_secs(60),
            webhook_alert_after: Duration::from_secs(5 * 60),
            webhook_fallback_after: Duration::from_secs(10 * 60),
            queue_recheck: Duration::from_secs(2),
            send: SendLimits::default(),
            queue_high: 4_096,
            queue_low: 512,
            chat_queue: 64,
            flood_report_every: Duration::from_secs(60),
            offset_every: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error(transparent)]
    Telegram(#[from] TelegramError),
    #[error(transparent)]
    Service(#[from] ServiceError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Module(#[from] ModuleError),
    #[error("функция бота ещё не реализована: {0}")]
    NotImplemented(&'static str),
    #[error("бот подключён вебхуком к другому адресу")]
    Webhook(Option<String>),
    #[error("этот токен уже опрашивает другая программа")]
    Conflict,
    #[error("бот уже запущен в другом процессе этой панели")]
    Duplicate,
}

enum Stop {
    Cancelled,
    TokenChanged,
    ProxyChanged,
    ModeChanged,
}

pub struct Bot {
    services: Services,
    config: BotConfig,
}

fn is_webhook_conflict(description: &str) -> bool {
    description.to_ascii_lowercase().contains("webhook")
}

fn webhook_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

fn diagnose(err: &BotError) -> (BotProblem, Option<String>, Duration) {
    match err {
        BotError::Telegram(TelegramError::Api {
            code: 401 | 404, ..
        }) => (BotProblem::BadToken, None, BAD_TOKEN_PAUSE),
        BotError::Telegram(TelegramError::Api {
            code: 409,
            description,
            ..
        }) if is_webhook_conflict(description) => (BotProblem::Webhook, None, WEBHOOK_PAUSE),
        BotError::Telegram(TelegramError::Api { code: 409, .. }) | BotError::Conflict => {
            (BotProblem::Conflict, None, RETRY_PAUSE)
        }
        BotError::Duplicate => (BotProblem::Duplicate, None, RETRY_PAUSE),
        BotError::Webhook(host) => (BotProblem::Webhook, host.clone(), WEBHOOK_PAUSE),
        BotError::Telegram(TelegramError::Transport(_)) => {
            (BotProblem::Unreachable, None, RETRY_PAUSE)
        }
        BotError::Telegram(TelegramError::Api { description, .. }) => {
            (BotProblem::Telegram, Some(description.clone()), RETRY_PAUSE)
        }
        BotError::Telegram(TelegramError::Decode(_)) => (BotProblem::Telegram, None, RETRY_PAUSE),
        BotError::Service(_)
        | BotError::Store(_)
        | BotError::Module(_)
        | BotError::NotImplemented(_) => (BotProblem::Internal, None, RETRY_PAUSE),
    }
}

fn later(services: &Services, pause: Duration) -> chrono::DateTime<chrono::Utc> {
    services.now() + chrono::Duration::from_std(pause).unwrap_or_else(|_| chrono::Duration::zero())
}

fn offset_for(value: &serde_json::Value, bot: i64, same_bot: bool) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) if same_bot => n.as_i64(),
        serde_json::Value::Object(map) if map.get("bot").and_then(|b| b.as_i64()) == Some(bot) => {
            map.get("offset").and_then(|o| o.as_i64())
        }
        _ => None,
    }
}

async fn load_offset(
    services: &Services,
    bot: i64,
    same_bot: bool,
) -> Result<Option<i64>, StoreError> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .bot_state()
        .get(OFFSET_KEY)
        .await?
        .and_then(|v| offset_for(&v, bot, same_bot)))
}

async fn remembered_username(services: &Services) -> Result<Option<String>, StoreError> {
    let mut conn = services.store().conn().await?;
    Ok(conn
        .settings()
        .get(keys::BOT_USERNAME)
        .await?
        .and_then(|v| v.as_str().map(str::to_string)))
}

async fn save_offset(services: &Services, bot: i64, offset: i64) -> Result<(), StoreError> {
    let mut conn = services.store().conn().await?;
    conn.bot_state()
        .set(
            OFFSET_KEY,
            &serde_json::json!({ "bot": bot, "offset": offset }),
        )
        .await
}

async fn remember_username(services: &Services, username: &str) -> Result<(), StoreError> {
    let mut conn = services.store().conn().await?;
    let stored = conn.settings().get(keys::BOT_USERNAME).await?;
    if stored.as_ref().and_then(|v| v.as_str()) != Some(username) {
        conn.settings()
            .set(keys::BOT_USERNAME, &serde_json::json!(username), None)
            .await?;
    }
    Ok(())
}

const RUSSIAN_CODES: [&str; 4] = ["ru", "uk", "be", "kk"];

struct Refused<T> {
    what: T,
    until: tokio::time::Instant,
}

impl<T: PartialEq> Refused<T> {
    fn new(what: T, err: &TelegramError) -> Self {
        let asked = Duration::from_secs(err.retry_after().unwrap_or_default());
        Refused {
            what,
            until: tokio::time::Instant::now() + asked.max(SYNC_RETRY),
        }
    }

    fn holds(refused: &Option<Self>, what: &T) -> bool {
        refused
            .as_ref()
            .is_some_and(|r| r.what == *what && tokio::time::Instant::now() < r.until)
    }
}

async fn sync_commands(
    api: &TelegramApi,
    services: &Services,
    applied: &mut Option<sn_domain::client::Languages>,
    refused: &mut Option<Refused<sn_domain::client::Languages>>,
) {
    let languages = services.settings().languages;
    if *applied == Some(languages) || Refused::holds(refused, &languages) {
        return;
    }
    let russian = [
        ("start", "Главное меню"),
        ("language", "Язык"),
        ("paysupport", "Вопросы по оплате звёздами"),
    ];
    let english = [
        ("start", "Main menu"),
        ("language", "Language"),
        ("paysupport", "Questions about paying with Stars"),
    ];
    let mut failed = None;
    if languages.several() {
        if let Err(err) = api.set_commands_for(&english, None).await {
            failed = Some(err);
        }
        for code in RUSSIAN_CODES {
            if let Err(err) = api.set_commands_for(&russian, Some(code)).await {
                failed = Some(err);
            }
        }
    } else {
        let only = [russian[0], russian[2]];
        if let Err(err) = api.set_commands_for(&only, None).await {
            failed = Some(err);
        }
        for code in RUSSIAN_CODES {
            if let Err(err) = api.delete_commands_for(code).await {
                failed = Some(err);
            }
        }
    }
    match failed {
        None => {
            *applied = Some(languages);
            *refused = None;
        }
        Some(err) => {
            tracing::debug!(error = %err, "команды бота не обновлены");
            *refused = Some(Refused::new(languages, &err));
        }
    }
}

async fn sync_menu(
    api: &TelegramApi,
    services: &Services,
    applied: &mut Option<String>,
    refused: &mut Option<Refused<Option<String>>>,
) {
    let buttons = services.bot_buttons();
    let wanted = buttons.wanted();
    let key = wanted.key();
    if key == *applied || Refused::holds(refused, &key) {
        return;
    }
    match api.set_menu_button(&wanted).await {
        Ok(_) => {
            *applied = key;
            *refused = None;
            if let Err(err) = buttons.remember(&wanted).await {
                tracing::warn!(error = %err, "кнопка меню бота обновлена, но не запомнена");
            }
        }
        Err(err) => {
            tracing::debug!(error = %err, "кнопка меню бота не обновлена");
            *refused = Some(Refused::new(key, &err));
        }
    }
}

fn spawn_workers(ctx: &BotCtx, lanes: &Arc<Lanes>) -> Vec<JoinHandle<()>> {
    (0..lanes.workers())
        .map(|_| {
            let ctx = ctx.clone();
            let lanes = lanes.clone();
            tokio::spawn(async move {
                while let Some((chat, update)) = lanes.next().await {
                    let ctx = ctx.clone();
                    let job = tokio::spawn(async move {
                        let _work = ctx.services.pause_gate().enter().await;
                        handle_update(&ctx, update).await;
                    });
                    if let Err(err) = job.await {
                        tracing::error!(error = %err, "обработка апдейта прервалась");
                    }
                    lanes.done(chat);
                }
            })
        })
        .collect()
}

fn group_chatter(update: &Update) -> bool {
    update.message.as_ref().is_some_and(|message| {
        message.chat.is_group()
            && message
                .text
                .as_deref()
                .and_then(handler::parse_command)
                .is_none_or(|(command, _)| command != "start")
    })
}

pub fn probe(update: &Update) -> Probe {
    let simple = |user: &BotUser, kind: Kind| Probe {
        user: Some(user.id),
        name: user.username.clone(),
        kind,
        start: None,
    };
    if let Some(query) = &update.pre_checkout_query {
        return simple(&query.from, Kind::Payment);
    }
    if let Some(callback) = &update.callback_query {
        return simple(&callback.from, Kind::Button);
    }
    if let Some(query) = &update.widget_query {
        return simple(&query.from, Kind::Button);
    }
    let Some(message) = &update.message else {
        return Probe {
            user: None,
            name: None,
            kind: Kind::Command,
            start: None,
        };
    };
    if message.successful_payment.is_some() || message.refunded_payment.is_some() {
        return Probe {
            user: message.from.as_ref().map(|u| u.id),
            name: None,
            kind: Kind::Payment,
            start: None,
        };
    }
    Probe::message(
        message
            .from
            .as_ref()
            .map(|u| u.id)
            .or(Some(message.chat.id)),
        message.from.as_ref().and_then(|u| u.username.clone()),
        message.text.as_deref(),
        message.has_photo() || message.document.is_some(),
    )
}

fn live_proxy_direct(
    api_base: &str,
    token: &str,
    proxy: &telegram::Proxy,
) -> Option<Arc<TelegramApi>> {
    proxy
        .as_ref()
        .map(|_| Arc::new(TelegramApi::new(api_base, token, None)))
}

struct Live<'a> {
    api: Arc<TelegramApi>,
    ctx: BotCtx,
    lanes: Arc<Lanes>,
    me: BotUser,
    queue: i64,
    token: &'a str,
    proxy: telegram::Proxy,
    since: chrono::DateTime<chrono::Utc>,
    menu: Option<String>,
    menu_refused: Option<Refused<Option<String>>>,
    commands: Option<sn_domain::client::Languages>,
    commands_refused: Option<Refused<sn_domain::client::Languages>>,
    next_lease: tokio::time::Instant,
    next_purge: tokio::time::Instant,
    next_report: tokio::time::Instant,
    signups_reported: Option<tokio::time::Instant>,
}

impl Live<'_> {
    fn running(&self, at: chrono::DateTime<chrono::Utc>) -> BotState {
        BotState::Running {
            username: self.me.username.clone(),
            since: self.since,
            last_ok_at: at,
        }
    }
}

impl Bot {
    pub fn new(services: Services, config: BotConfig) -> Self {
        Bot { services, config }
    }

    fn telegram_proxy(&self) -> telegram::Proxy {
        self.services
            .egress()
            .proxy_for(sn_egress::Route::Telegram, None)
    }

    pub async fn run(self, cancel: CancellationToken) -> Result<(), BotError> {
        let runtime = self.services.bot_runtime();
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let Some(token) = self.services.bot_token() else {
                if runtime.state() != BotState::NoToken {
                    tracing::info!("бот выключен: не задан токен");
                    runtime.set(BotState::NoToken);
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = runtime.woken() => {}
                    _ = tokio::time::sleep(IDLE_RECHECK) => {}
                }
                continue;
            };
            runtime.set(BotState::Starting);
            match self.serve(&token, &cancel).await {
                Ok(Stop::Cancelled) => return Ok(()),
                Ok(Stop::TokenChanged) => {
                    tracing::info!("токен бота сменился, переподключаемся");
                }
                Ok(Stop::ProxyChanged) => {
                    tracing::info!("прокси для Telegram сменился, переподключаемся");
                }
                Ok(Stop::ModeChanged) => {
                    tracing::info!("способ получения обновлений сменился, переподключаемся");
                }
                Err(err) => {
                    let (problem, detail, pause) = diagnose(&err);
                    runtime.set(BotState::Failed {
                        problem,
                        detail,
                        at: self.services.now(),
                        retry_at: Some(later(&self.services, pause)),
                    });
                    tracing::warn!(error = %err, ?problem, "бот не запустился, повторим позже");
                    alerts::flush_proxy(&self.services, &self.config.api_base, &token).await;
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = runtime.woken() => {}
                        _ = tokio::time::sleep(pause) => {}
                    }
                }
            }
        }
    }

    async fn serve(&self, token: &str, cancel: &CancellationToken) -> Result<Stop, BotError> {
        let mut lease = webhook::Lease::acquire(&self.services).await?;
        let outcome = self.serve_leased(token, cancel, &mut lease).await;
        lease.release().await;
        outcome
    }

    async fn serve_leased(
        &self,
        token: &str,
        cancel: &CancellationToken,
        lease: &mut webhook::Lease,
    ) -> Result<Stop, BotError> {
        let runtime = self.services.bot_runtime();
        let takeover = runtime.take_takeover();
        let proxy = self.telegram_proxy();
        let mut api = TelegramApi::paced(
            &self.config.api_base,
            token,
            self.config.send,
            proxy.clone(),
        );
        if self.services.env().telegram_api.is_none() {
            api = api.timed(self.services.server_clock().clone());
        }
        let api = Arc::new(api);
        let me = api.get_me().await?;
        let previous = remembered_username(&self.services).await?;
        let same_bot = previous.is_some() && previous == me.username;
        if let Some(username) = me.username.as_deref()
            && let Err(err) = remember_username(&self.services, username).await
        {
            tracing::warn!(error = %err, "имя бота не сохранено");
        }
        let hooks = self.services.bot_webhook();
        let wanted = hooks.wanted();
        let hook = match api.get_webhook_info().await {
            Ok(hook) => Some(hook),
            Err(err) => {
                tracing::debug!(error = %err, "сведения о вебхуке бота не получены");
                None
            }
        };
        let foreign = match hook.as_ref().filter(|h| !h.url.is_empty()) {
            Some(h) if !hooks.is_ours(&h.url).await? => Some(h.url.clone()),
            _ => None,
        };
        if let Some(url) = &foreign
            && !takeover
        {
            return Err(BotError::Webhook(webhook_host(url)));
        }
        let queue = sn_services::bot_webhook::bot_id(token).unwrap_or(me.id);
        let target = if wanted {
            match webhook::register(&self.services, &api, queue, hook.as_ref()).await? {
                webhook::Setup::Ready(url) => Some(url),
                webhook::Setup::Rejected(description) => {
                    webhook::fall_back_rejected(&self.services, description).await?;
                    return Ok(Stop::ModeChanged);
                }
            }
        } else {
            let set = hook.as_ref().is_some_and(|h| !h.url.is_empty());
            if set || (hook.is_none() && takeover) {
                api.delete_webhook().await?;
                if foreign.is_some() || takeover {
                    tracing::info!("вебхук бота снят: администратор забрал бота сюда");
                } else {
                    tracing::info!("вебхук бота снят, бот получает обновления опросом");
                }
            }
            None
        };
        let mut commands = None;
        let mut commands_refused = None;
        sync_commands(&api, &self.services, &mut commands, &mut commands_refused).await;
        tracing::info!(
            username = %me.username.clone().unwrap_or_default(),
            mode = if target.is_some() { "webhook" } else { "polling" },
            "бот подключён"
        );
        let ctx = BotCtx {
            services: self.services.clone(),
            api: api.clone(),
            username: me.username.clone(),
        };
        let lanes = Arc::new(Lanes::new(
            self.config.workers,
            self.config.queue_high,
            self.config.queue_low,
            self.config.chat_queue,
        ));
        let handles = spawn_workers(&ctx, &lanes);
        let keeper_stop = cancel.child_token();
        let keeper_guard = keeper_stop.clone().drop_guard();
        let keeper = tokio::spawn(
            stars::Keeper::new(
                self.services.clone(),
                &self.config.api_base,
                token,
                me.id,
                self.config.stars_retry_every,
                self.config.stars_reconcile_every,
            )
            .run(keeper_stop),
        );
        let alerts_stop = cancel.child_token();
        let alerts_guard = alerts_stop.clone().drop_guard();
        let direct = live_proxy_direct(&self.config.api_base, token, &self.telegram_proxy());
        let alerts_keeper = tokio::spawn(
            alerts::Keeper::new(
                self.services.clone(),
                api.clone(),
                self.config.alerts_tick,
                self.config.alerts_every,
            )
            .with_direct(direct)
            .run(alerts_stop),
        );
        let now = tokio::time::Instant::now();
        let mut live = Live {
            api,
            ctx,
            lanes: lanes.clone(),
            me,
            queue,
            token,
            proxy,
            since: self.services.now(),
            menu: self
                .services
                .bot_buttons()
                .stored()
                .await
                .unwrap_or_default(),
            menu_refused: None,
            commands,
            commands_refused,
            next_lease: now + LEASE_CHECK,
            next_purge: now,
            next_report: now + self.config.flood_report_every,
            signups_reported: None,
        };
        runtime.set(live.running(live.since));
        let outcome = match target {
            Some(url) => self.consume(&mut live, url, lease, cancel).await,
            None => {
                let offset = load_offset(&self.services, live.me.id, same_bot).await;
                match offset {
                    Ok(offset) => self.poll(&mut live, offset, lease, cancel).await,
                    Err(err) => Err(err.into()),
                }
            }
        };
        drop(live);
        lanes.close();
        drop(keeper_guard);
        drop(alerts_guard);
        for handle in handles {
            if tokio::time::timeout(Duration::from_secs(8), handle)
                .await
                .is_err()
            {
                tracing::warn!("обработчик апдейтов не завершился за 8 секунд");
            }
        }
        if tokio::time::timeout(Duration::from_secs(8), keeper)
            .await
            .is_err()
        {
            tracing::warn!("сверка оплат звёздами не завершилась за 8 секунд");
        }
        if tokio::time::timeout(Duration::from_secs(8), alerts_keeper)
            .await
            .is_err()
        {
            tracing::warn!("уведомления администраторам не остановились за 8 секунд");
        }
        outcome
    }

    async fn drain(&self, live: &Live<'_>) -> usize {
        let mut total = 0;
        loop {
            match webhook::take_batch(&self.services, live.queue).await {
                Ok(updates) => {
                    let count = updates.len();
                    for update in updates {
                        self.dispatch(live, update).await;
                    }
                    total += count;
                    if count < webhook::BATCH as usize {
                        return total;
                    }
                }
                Err(err) => {
                    tracing::warn!(error = %err, "обновления из очереди вебхука не взяты, повторим");
                    return total;
                }
            }
        }
    }

    async fn dispatch(&self, live: &Live<'_>, mut update: Update) {
        if update.pre_checkout_query.is_some() {
            let ctx = live.ctx.clone();
            tokio::spawn(async move {
                let _work = ctx.services.pause_gate().enter().await;
                handle_update(&ctx, update).await
            });
            return;
        }
        if group_chatter(&update) {
            return;
        }
        let probe = probe(&update);
        let settings = self.services.settings().bot_flood;
        match self
            .services
            .bot_flood()
            .check(&probe, &settings, std::time::Instant::now())
        {
            Verdict::Admit { lane, fresh } => {
                update.repeat = !fresh;
                live.lanes.push(update, lane).await;
            }
            Verdict::Refuse(Refusal::JustBlocked(blocked)) => {
                tracing::info!(
                    user = blocked.user,
                    updates = blocked.updates,
                    minutes = settings.block_minutes,
                    "аккаунт временно заблокирован за флуд"
                );
                let private = update
                    .message
                    .as_ref()
                    .map(|m| m.chat.is_private())
                    .unwrap_or(true);
                if private {
                    let ctx = live.ctx.clone();
                    let chat = blocked.user;
                    tokio::spawn(async move {
                        let content = ctx
                            .services
                            .bot_content_in(ctx.services.settings().languages.fallback());
                        let text = ui::text(&content, "common.flood", &[]);
                        if let Err(err) = ctx.api.send_spare(chat, &text).await {
                            tracing::debug!(error = %err, "предупреждение о флуде не отправлено");
                        }
                    });
                }
            }
            Verdict::Refuse(_) => {}
        }
    }

    async fn report_flood(&self, live: &mut Live<'_>) {
        let report = self.services.bot_flood().report(std::time::Instant::now());
        let lanes = live.lanes.stats();
        let settings = self.services.settings().bot_flood;
        let hooks = self.services.bot_webhook();
        if report.blocked_new > 0 {
            let accounts: Vec<serde_json::Value> = report
                .blocked
                .iter()
                .filter_map(|b| serde_json::to_value(b).ok())
                .collect();
            if let Err(err) = hooks
                .incident(
                    sn_services::bot_flood::BLOCK_ACTION,
                    serde_json::json!({
                        "blocked": report.blocked_new,
                        "block_minutes": settings.block_minutes,
                        "accounts": accounts,
                    }),
                )
                .await
            {
                tracing::warn!(error = %err, "блокировки за флуд не записаны");
            }
        }
        let now = tokio::time::Instant::now();
        let quiet = live
            .signups_reported
            .is_none_or(|at| now.saturating_duration_since(at) >= SIGNUPS_REPORT_EVERY);
        if report.refused > 0 && quiet {
            live.signups_reported = Some(now);
            let antibot = self
                .services
                .modules()
                .is_enabled(sn_domain::module::ModuleId::Antibot);
            if let Err(err) = hooks
                .incident(
                    sn_services::bot_flood::SIGNUPS_ACTION,
                    serde_json::json!({
                        "accepted": report.signups,
                        "refused": report.refused,
                        "limit": settings.registrations_per_minute,
                        "antibot": antibot,
                    }),
                )
                .await
            {
                tracing::warn!(error = %err, "наплыв регистраций не записан");
            }
        }
        if report.dropped > 0 || report.refused > 0 || lanes.overflow > 0 {
            tracing::info!(
                admitted = report.admitted,
                dropped = report.dropped,
                overflow = lanes.overflow,
                blocked_new = report.blocked_new,
                blocked_now = report.blocked_now,
                signups = report.signups,
                refused = report.refused,
                queue_peak = lanes.peak,
                users = report.users,
                "защита от флуда за минуту"
            );
        }
        let mut snapshot = sn_services::bot_flood::FloodSnapshot::of(&report, self.services.now());
        snapshot.dropped += lanes.overflow;
        let saved = match serde_json::to_value(&snapshot) {
            Ok(value) => match self.services.store().conn().await {
                Ok(mut conn) => conn
                    .bot_state()
                    .set(sn_services::bot_flood::STATE_KEY, &value)
                    .await
                    .map_err(|e| e.to_string()),
                Err(err) => Err(err.to_string()),
            },
            Err(err) => Err(err.to_string()),
        };
        if let Err(err) = saved {
            tracing::debug!(error = %err, "сводка защиты от флуда не сохранена");
        }
    }

    async fn upkeep(
        &self,
        live: &mut Live<'_>,
        lease: &mut webhook::Lease,
    ) -> Result<(), BotError> {
        let now = tokio::time::Instant::now();
        if now >= live.next_report {
            live.next_report = now + self.config.flood_report_every;
            self.report_flood(live).await;
        }
        if now >= live.next_lease {
            lease.alive().await?;
            live.next_lease = now + LEASE_CHECK;
        }
        if now >= live.next_purge {
            live.next_purge = now + PURGE_EVERY;
            match self.services.bot_webhook().purge(live.queue).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "старые записи очереди вебхука удалены"),
                Err(err) => tracing::warn!(error = %err, "очередь вебхука не почищена"),
            }
        }
        Ok(())
    }

    async fn sync(&self, api: &TelegramApi, live: &mut Live<'_>) {
        sync_menu(api, &self.services, &mut live.menu, &mut live.menu_refused).await;
        sync_commands(
            api,
            &self.services,
            &mut live.commands,
            &mut live.commands_refused,
        )
        .await;
    }

    async fn poll(
        &self,
        live: &mut Live<'_>,
        mut offset: Option<i64>,
        lease: &mut webhook::Lease,
        cancel: &CancellationToken,
    ) -> Result<Stop, BotError> {
        let runtime = self.services.bot_runtime();
        let api = live.api.clone();
        self.drain(live).await;
        let mut failures = 0u32;
        let mut saved_at = tokio::time::Instant::now();
        let mut unsaved: Option<i64> = None;
        let outcome = loop {
            if self.services.bot_token().as_deref() != Some(live.token) {
                break Ok(Stop::TokenChanged);
            }
            if self.telegram_proxy() != live.proxy {
                break Ok(Stop::ProxyChanged);
            }
            if self.services.bot_webhook().wanted() {
                break Ok(Stop::ModeChanged);
            }
            if self.services.pause_gate().is_paused() {
                tokio::select! {
                    _ = cancel.cancelled() => break Ok(Stop::Cancelled),
                    _ = self.services.pause_gate().cleared() => {}
                }
                continue;
            }
            self.upkeep(live, lease).await?;
            self.sync(&api, live).await;
            let polled = tokio::select! {
                _ = cancel.cancelled() => break Ok(Stop::Cancelled),
                _ = runtime.woken() => continue,
                _ = self.services.bot_updates_signal().notified() => {
                    self.drain(live).await;
                    continue;
                }
                polled = api.get_updates(offset, self.config.poll_timeout) => polled,
            };
            match polled {
                Ok(updates) => {
                    failures = 0;
                    runtime.set(live.running(self.services.now()));
                    if !updates.is_empty() {
                        let payments = stars::from_updates(&updates);
                        if !payments.is_empty()
                            && let Err(err) = stars::record(&self.services, &payments).await
                        {
                            tracing::warn!(
                                error = %err,
                                "оплаты звёздами не сохранены, апдейты будут прочитаны заново"
                            );
                            tokio::select! {
                                _ = cancel.cancelled() => break Ok(Stop::Cancelled),
                                _ = tokio::time::sleep(SAVE_RETRY) => {}
                            }
                            continue;
                        }
                        let mut next = offset.unwrap_or(0);
                        for update in updates {
                            next = next.max(update.update_id + 1);
                            self.dispatch(live, update).await;
                        }
                        offset = Some(next);
                        unsaved = Some(next);
                    }
                    if let Some(next) = unsaved
                        && saved_at.elapsed() >= self.config.offset_every
                    {
                        saved_at = tokio::time::Instant::now();
                        match save_offset(&self.services, live.me.id, next).await {
                            Ok(()) => unsaved = None,
                            Err(err) => {
                                tracing::warn!(error = %err, "смещение апдейтов не сохранено")
                            }
                        }
                    }
                }
                Err(err) => {
                    match &err {
                        TelegramError::Api {
                            code: 401 | 404, ..
                        } => break Err(err.into()),
                        TelegramError::Api {
                            code: 409,
                            description,
                            ..
                        } if is_webhook_conflict(description) => {
                            break Err(BotError::Webhook(None));
                        }
                        TelegramError::Api { code: 409, .. } => break Err(BotError::Conflict),
                        _ => {}
                    }
                    failures += 1;
                    let pause = Duration::from_secs(err.retry_after().unwrap_or(3));
                    if failures >= UNREACHABLE_AFTER {
                        let (problem, detail) = match &err {
                            TelegramError::Transport(_) => (BotProblem::Unreachable, None),
                            TelegramError::Api { description, .. } => {
                                (BotProblem::Telegram, Some(description.clone()))
                            }
                            TelegramError::Decode(_) => (BotProblem::Telegram, None),
                        };
                        runtime.set(BotState::Failed {
                            problem,
                            detail,
                            at: self.services.now(),
                            retry_at: Some(later(&self.services, pause)),
                        });
                    }
                    tracing::warn!(error = %err, "getUpdates не удался");
                    tokio::select! {
                        _ = cancel.cancelled() => break Ok(Stop::Cancelled),
                        _ = tokio::time::sleep(pause) => {}
                    }
                }
            }
        };
        if let Some(next) = unsaved
            && let Err(err) = save_offset(&self.services, live.me.id, next).await
        {
            tracing::warn!(error = %err, "смещение апдейтов не сохранено при остановке");
        }
        outcome
    }

    async fn consume(
        &self,
        live: &mut Live<'_>,
        mut url: String,
        lease: &mut webhook::Lease,
        cancel: &CancellationToken,
    ) -> Result<Stop, BotError> {
        let runtime = self.services.bot_runtime();
        let api = live.api.clone();
        let hooks = self.services.bot_webhook();
        let mut next_check =
            tokio::time::Instant::now() + self.config.webhook_check_every.min(FIRST_CHECK);
        loop {
            if self.services.bot_token().as_deref() != Some(live.token) {
                webhook::forget_previous(&self.services, &api).await;
                return Ok(Stop::TokenChanged);
            }
            if self.telegram_proxy() != live.proxy {
                return Ok(Stop::ProxyChanged);
            }
            if !hooks.wanted() {
                return Ok(Stop::ModeChanged);
            }
            if self.services.pause_gate().is_paused() {
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(Stop::Cancelled),
                    _ = self.services.pause_gate().cleared() => {}
                }
                continue;
            }
            self.upkeep(live, lease).await?;
            self.sync(&api, live).await;
            if hooks.target().is_some_and(|(wanted, _)| wanted != url) {
                match webhook::register(&self.services, &api, live.queue, None).await? {
                    webhook::Setup::Ready(fresh) => url = fresh,
                    webhook::Setup::Rejected(description) => {
                        webhook::fall_back_rejected(&self.services, description).await?;
                        return Ok(Stop::ModeChanged);
                    }
                }
            }
            if self.drain(live).await > 0 {
                runtime.set(live.running(self.services.now()));
            }
            if tokio::time::Instant::now() >= next_check {
                next_check = tokio::time::Instant::now() + self.config.webhook_check_every;
                match webhook::check(&self.services, &api, &url, &self.config).await? {
                    webhook::Verdict::Fine => runtime.set(live.running(self.services.now())),
                    webhook::Verdict::Unknown => {}
                    webhook::Verdict::Lost => {
                        match webhook::register(&self.services, &api, live.queue, None).await? {
                            webhook::Setup::Ready(fresh) => url = fresh,
                            webhook::Setup::Rejected(description) => {
                                webhook::fall_back_rejected(&self.services, description).await?;
                                return Ok(Stop::ModeChanged);
                            }
                        }
                    }
                    webhook::Verdict::Foreign(host) => return Err(BotError::Webhook(host)),
                    webhook::Verdict::FellBack => return Ok(Stop::ModeChanged),
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => return Ok(Stop::Cancelled),
                _ = runtime.woken() => {}
                _ = self.services.bot_updates_signal().notified() => {}
                _ = tokio::time::sleep(self.config.queue_recheck) => {}
            }
        }
    }
}
