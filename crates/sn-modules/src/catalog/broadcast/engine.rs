use super::letter::{self, Letter};
use super::store::{self, Claimed, ImageData, Lease, Message, Outcome};
use super::{ButtonSpec, Lane, MediaKind, PREVIEW_TOKEN, Status, audit, letter_for};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::sender::{
    BotSender, Delivery, MailDelivery, MailOut, MailSender, MailSite, Media, MediaFile, OutButton,
    Outgoing,
};
use sn_domain::client::Locale;
use sn_domain::lang;
use sn_store::Db;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;
use uuid::Uuid;

pub const LEASE_KEY: &str = "broadcast.engine";
pub const MAIL_LEASE_KEY: &str = "broadcast.mail";
const DUE_BATCH: i64 = 5;
const STALE_FLIGHT_SECS: i64 = 300;
const CHAT_MEMORY: usize = 4096;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub per_second: u32,
    pub chat_gap: Duration,
    pub in_flight: usize,
    pub idle: Duration,
    pub retries: i32,
    pub retry_pause: Duration,
    pub bot_pause: Duration,
    pub lease: Duration,
    pub grace: Duration,
    pub reject_limit: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            per_second: 25,
            chat_gap: Duration::from_secs(1),
            in_flight: 8,
            idle: Duration::from_secs(2),
            retries: 3,
            retry_pause: Duration::from_secs(5),
            bot_pause: Duration::from_secs(30),
            lease: Duration::from_secs(20),
            grace: Duration::from_secs(8),
            reject_limit: 10,
        }
    }
}

impl EngineConfig {
    pub fn mail() -> Self {
        EngineConfig {
            per_second: 2,
            chat_gap: Duration::ZERO,
            in_flight: 2,
            idle: Duration::from_secs(2),
            retries: 3,
            retry_pause: Duration::from_secs(15),
            bot_pause: Duration::from_secs(60),
            lease: Duration::from_secs(30),
            grace: Duration::from_secs(25),
            reject_limit: 10,
        }
    }
}

#[derive(Clone)]
pub enum Sender {
    Bot(Arc<dyn BotSender>),
    Mail(Arc<dyn MailSender>),
}

impl Sender {
    fn lane(&self) -> Lane {
        match self {
            Sender::Bot(_) => Lane::Bot,
            Sender::Mail(_) => Lane::Email,
        }
    }

    fn lease_key(&self) -> &'static str {
        match self {
            Sender::Bot(_) => LEASE_KEY,
            Sender::Mail(_) => MAIL_LEASE_KEY,
        }
    }

    fn bot_id(&self) -> Option<i64> {
        match self {
            Sender::Bot(sender) => sender.bot_id(),
            Sender::Mail(_) => None,
        }
    }
}

struct Pace {
    spacing: Duration,
    gap: Duration,
    next: Instant,
    paused_until: Instant,
    chats: HashMap<i64, Instant>,
}

impl Pace {
    fn new(config: &EngineConfig) -> Self {
        let now = Instant::now();
        Pace {
            spacing: Duration::from_secs(1) / config.per_second.max(1),
            gap: config.chat_gap,
            next: now,
            paused_until: now,
            chats: HashMap::new(),
        }
    }

    fn wait(&self) -> Duration {
        self.next
            .max(self.paused_until)
            .saturating_duration_since(Instant::now())
    }

    fn take(&mut self) -> Duration {
        let now = Instant::now();
        let slot = self.next.max(self.paused_until).max(now);
        self.next = slot + self.spacing;
        slot.saturating_duration_since(now)
    }

    fn pause(&mut self, pause: Duration) {
        self.paused_until = self.paused_until.max(Instant::now() + pause);
    }

    fn chat(&mut self, chat: i64) -> Duration {
        let now = Instant::now();
        if self.chats.len() > CHAT_MEMORY {
            let gap = self.gap;
            self.chats.retain(|_, at| *at + gap > now);
        }
        let ready = self
            .chats
            .get(&chat)
            .map_or(now, |at| (*at + self.gap).max(now));
        self.chats.insert(chat, ready);
        ready.saturating_duration_since(now)
    }
}

enum Source {
    Text,
    Cached(Arc<ImageData>, Arc<str>),
    Upload(Arc<ImageData>),
}

impl Source {
    fn media(&self) -> Option<Media<'_>> {
        match self {
            Source::Text => None,
            Source::Cached(image, file_id) => Some(image.media(MediaFile::Cached(file_id))),
            Source::Upload(image) => Some(image.upload()),
        }
    }
}

struct BotMessage {
    text: Arc<str>,
    buttons: Arc<[OutButton]>,
    image: Option<Arc<ImageData>>,
    file: Option<(i64, Arc<str>)>,
}

impl BotMessage {
    fn new(message: Message) -> Self {
        let file = message.image.as_ref().and_then(|image| {
            image
                .file_bot
                .zip(image.file_id.as_deref())
                .map(|(bot, file)| (bot, Arc::from(file)))
        });
        BotMessage {
            text: Arc::from(message.text),
            buttons: message.buttons.iter().map(ButtonSpec::out).collect(),
            image: message.image.map(Arc::new),
            file,
        }
    }

    fn source(&self, bot: Option<i64>) -> Source {
        let Some(image) = &self.image else {
            return Source::Text;
        };
        match (&self.file, bot) {
            (Some((file_bot, file)), Some(bot)) if *file_bot == bot => {
                Source::Cached(image.clone(), file.clone())
            }
            _ => Source::Upload(image.clone()),
        }
    }

    fn missing(&self, bot: Option<i64>) -> Option<i64> {
        match self.source(bot) {
            Source::Upload(image) if !image.loaded() => Some(image.id),
            _ => None,
        }
    }

    fn fill(&mut self, bytes: Vec<u8>) {
        if let Some(image) = &self.image {
            let mut full = ImageData::clone(image);
            full.bytes = bytes;
            self.image = Some(Arc::new(full));
        }
    }

    fn cache(&mut self, bot: i64, file_id: String) {
        self.file = Some((bot, Arc::from(file_id)));
        if let Some(image) = &self.image
            && image.loaded()
        {
            let mut light = ImageData::clone(image);
            light.bytes = Vec::new();
            self.image = Some(Arc::new(light));
        }
    }
}

struct MailMessage {
    letter: Arc<Letter>,
    site: Arc<MailSite>,
}

enum Body {
    Bot(BotMessage),
    Mail(MailMessage),
}

struct Prepared {
    id: i64,
    body: Body,
    rejected: u32,
    notice: bool,
    automatic: bool,
}

impl Prepared {
    fn uploading(&self, bot: Option<i64>) -> bool {
        match &self.body {
            Body::Bot(message) => matches!(message.source(bot), Source::Upload(_)),
            Body::Mail(_) => false,
        }
    }
}

enum Answer {
    Bot(Delivery),
    Mail(MailDelivery),
}

struct Flight {
    recipient: Claimed,
    answer: Answer,
    cached: bool,
}

struct State {
    pace: Pace,
    lead: Option<Instant>,
    prepared: Option<Prepared>,
}

pub struct Engine {
    ctx: ModuleContext,
    sender: Sender,
    enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    config: EngineConfig,
    owner: String,
}

impl Engine {
    pub fn new(
        ctx: ModuleContext,
        sender: Sender,
        enabled: impl Fn() -> bool + Send + Sync + 'static,
        config: EngineConfig,
    ) -> Self {
        Engine {
            ctx,
            sender,
            enabled: Arc::new(enabled),
            config,
            owner: Uuid::new_v4().to_string(),
        }
    }

    fn lane(&self) -> Lane {
        self.sender.lane()
    }

    fn held(&self) -> bool {
        self.ctx.core.core_settings().maintenance.enabled
    }

    pub async fn run(self, shutdown: impl Future<Output = ()> + Send + 'static) {
        let (signal, mut stop) = watch::channel(false);
        tokio::spawn(async move {
            shutdown.await;
            let _ = signal.send(true);
        });
        let mut state = State {
            pace: Pace::new(&self.config),
            lead: None,
            prepared: None,
        };
        let lane = self.lane().as_str();
        tracing::info!(lane, "рассылки: отправка запущена");
        loop {
            if *stop.borrow() {
                break;
            }
            if self.ctx.core.paused() {
                tokio::select! {
                    _ = stop.changed() => {}
                    _ = tokio::time::sleep(self.config.idle) => {}
                }
                continue;
            }
            let pause = match self.step(&mut state, &mut stop).await {
                Ok(pause) => pause,
                Err(err) => {
                    tracing::warn!(lane, error = %err, "рассылки: отправка споткнулась, повторим");
                    state.lead = None;
                    state.prepared = None;
                    self.config.idle * 3
                }
            };
            if pause.is_zero() {
                continue;
            }
            tokio::select! {
                _ = stop.changed() => {}
                _ = tokio::time::sleep(pause) => {}
            }
        }
        if state.lead.is_some()
            && let Ok(mut conn) = self.ctx.store.conn().await
            && let Err(err) =
                store::drop_lease(&mut conn, self.sender.lease_key(), &self.owner).await
        {
            tracing::debug!(error = %err, "рассылки: аренда отправки не снята");
        }
        tracing::info!(lane, "рассылки: отправка остановлена");
    }

    async fn lead(&self, lead: &mut Option<Instant>) -> Result<bool, ModuleError> {
        let now = Instant::now();
        if let Some(until) = *lead
            && until > now + self.config.lease / 2
        {
            return Ok(true);
        }
        let mut conn = self.ctx.store.conn().await?;
        let taken = store::take_lease(
            &mut conn,
            &Lease {
                key: self.sender.lease_key(),
                owner: &self.owner,
                seconds: self.config.lease.as_secs_f64(),
            },
        )
        .await?;
        if !taken {
            *lead = None;
            return Ok(false);
        }
        if lead.is_none() {
            let abandoned = store::abandon_flights(&mut conn, self.lane(), None).await?;
            if abandoned > 0 {
                tracing::warn!(
                    abandoned,
                    lane = self.lane().as_str(),
                    "рассылки: сообщения, которые отправлялись до перезапуска, не повторяются"
                );
            }
        }
        *lead = Some(now + self.config.lease);
        Ok(true)
    }

    async fn step(
        &self,
        state: &mut State,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Duration, ModuleError> {
        if !(self.enabled)() || !self.lead(&mut state.lead).await? {
            return Ok(self.config.idle);
        }
        let held = self.held();
        if !held {
            self.start_due().await?;
        }
        let mut conn = self.ctx.store.conn().await?;
        for id in store::complete_finished(&mut conn).await? {
            tracing::info!(broadcast = id, "рассылка отправлена");
        }
        let Some(id) = store::next_sending(&mut conn, self.lane(), held).await? else {
            state.prepared = None;
            return Ok(self.config.idle);
        };
        let prepared = match state.prepared.take() {
            Some(prepared) if prepared.id == id => prepared,
            _ => match self.prepare(&mut conn, id).await? {
                Some(prepared) => prepared,
                None => return Ok(self.config.bot_pause),
            },
        };
        drop(conn);
        let mut prepared = prepared;
        let pumped = self
            .pump(&mut prepared, &mut state.pace, &mut state.lead, stop)
            .await;
        state.prepared = Some(prepared);
        pumped
    }

    async fn prepare(
        &self,
        conn: &mut sn_store::Conn,
        id: i64,
    ) -> Result<Option<Prepared>, ModuleError> {
        let body = match &self.sender {
            Sender::Bot(_) => Body::Bot(BotMessage::new(store::message(conn, id, false).await?)),
            Sender::Mail(mail) => {
                let site = match mail.site() {
                    Ok(site) => site,
                    Err(reason) => {
                        let notice = format!(
                            "Почта не отправляет: {reason}. Письма уйдут, когда почта заработает"
                        );
                        store::set_notice(conn, id, Lane::Email, Some(&notice)).await?;
                        return Ok(None);
                    }
                };
                let message = store::message(conn, id, false).await?;
                let letter = letter_for(
                    message.subject.as_deref().unwrap_or_default(),
                    &message.text,
                    message
                        .image
                        .as_ref()
                        .filter(|image| image.kind() == MediaKind::Photo)
                        .map(|image| image.public_key),
                    &message.buttons,
                    &site,
                );
                Body::Mail(MailMessage {
                    letter: Arc::new(letter),
                    site: Arc::new(site),
                })
            }
        };
        Ok(Some(Prepared {
            id,
            body,
            rejected: 0,
            notice: true,
            automatic: store::automatic(conn, id).await?,
        }))
    }

    async fn start_due(&self) -> Result<(), ModuleError> {
        let mut tx = self.ctx.store.begin().await?;
        let due = store::due(&mut tx, DUE_BATCH).await?;
        if due.is_empty() {
            tx.rollback().await?;
            return Ok(());
        }
        let now = crate::catalog::db_now(&mut tx).await?;
        for (id, audience, channel) in due {
            let total = store::launch(&mut tx, id, &audience, channel, now, None).await?;
            if total == 0 {
                store::finish_empty(&mut tx, id).await?;
            }
            tx.audit()
                .record(&audit(
                    "broadcast.started",
                    None,
                    Some(id),
                    serde_json::json!({
                        "total": total,
                        "scheduled": true,
                        "audience": audience,
                        "channel": channel,
                    }),
                ))
                .await?;
            tracing::info!(broadcast = id, total, "рассылка по расписанию начата");
        }
        tx.commit().await?;
        Ok(())
    }

    async fn pump(
        &self,
        prepared: &mut Prepared,
        pace: &mut Pace,
        lead: &mut Option<Instant>,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Duration, ModuleError> {
        let mut flights: JoinSet<Flight> = JoinSet::new();
        let driven = self.drive(prepared, pace, lead, stop, &mut flights).await;
        match driven {
            Ok(stopping) => self.after(prepared.id, stopping).await,
            Err(err) => {
                flights.abort_all();
                *lead = None;
                Err(err)
            }
        }
    }

    async fn drive(
        &self,
        prepared: &mut Prepared,
        pace: &mut Pace,
        lead: &mut Option<Instant>,
        stop: &mut watch::Receiver<bool>,
        flights: &mut JoinSet<Flight>,
    ) -> Result<bool, ModuleError> {
        let id = prepared.id;
        let mut exhausted = false;
        let mut stopping_since: Option<Instant> = None;
        loop {
            while let Some(joined) = flights.try_join_next() {
                self.settle(id, joined, prepared, pace).await?;
            }
            if stopping_since.is_none()
                && (*stop.borrow()
                    || !(self.enabled)()
                    || (prepared.automatic && self.held())
                    || !self.lead(lead).await?)
            {
                stopping_since = Some(Instant::now());
            }
            if let Some(since) = stopping_since
                && since.elapsed() > self.config.grace
            {
                if !flights.is_empty() {
                    tracing::warn!(
                        left = flights.len(),
                        "рассылки: не дождались ответа при остановке"
                    );
                    flights.abort_all();
                }
                return Ok(true);
            }
            let limit = if prepared.uploading(self.sender.bot_id()) {
                1
            } else {
                self.config.in_flight.max(1)
            };
            if stopping_since.is_some() || exhausted || flights.len() >= limit {
                if flights.is_empty() {
                    return Ok(stopping_since.is_some());
                }
                let grace = stopping_since.map_or(self.config.idle, |since| {
                    self.config.grace.saturating_sub(since.elapsed())
                });
                tokio::select! {
                    joined = flights.join_next() => {
                        if let Some(joined) = joined {
                            self.settle(id, joined, prepared, pace).await?;
                        }
                    }
                    _ = tokio::time::sleep(grace) => {}
                }
                continue;
            }
            let wait = pace.wait();
            if !wait.is_zero() {
                tokio::select! {
                    joined = flights.join_next(), if !flights.is_empty() => {
                        if let Some(joined) = joined {
                            self.settle(id, joined, prepared, pace).await?;
                        }
                    }
                    _ = tokio::time::sleep(wait.min(Duration::from_secs(1))) => {}
                    _ = stop.changed() => {}
                }
                continue;
            }
            if let Body::Bot(message) = &mut prepared.body
                && let Some(image) = message.missing(self.sender.bot_id())
            {
                let mut conn = self.ctx.store.conn().await?;
                message.fill(store::image_bytes(&mut conn, image).await?);
            }
            let claimed = {
                let mut conn = self.ctx.store.conn().await?;
                store::claim(&mut conn, id, self.lane(), 1).await?
            };
            if claimed.is_empty() {
                exhausted = true;
            }
            for recipient in claimed {
                let slot = pace.take();
                let delay = match recipient.chat_id {
                    Some(chat) if !self.config.chat_gap.is_zero() => pace.chat(chat).max(slot),
                    _ => slot,
                };
                flights.spawn(self.flight(prepared, recipient, delay));
            }
        }
    }

    fn flight(
        &self,
        prepared: &Prepared,
        recipient: Claimed,
        delay: Duration,
    ) -> impl Future<Output = Flight> + Send + 'static {
        enum Job {
            Bot {
                sender: Arc<dyn BotSender>,
                text: Arc<str>,
                buttons: Arc<[OutButton]>,
                source: Source,
            },
            Mail {
                sender: Arc<dyn MailSender>,
                letter: Arc<Letter>,
                site: Arc<MailSite>,
            },
        }
        let job = match (&self.sender, &prepared.body) {
            (Sender::Bot(sender), Body::Bot(message)) => Some(Job::Bot {
                sender: sender.clone(),
                text: message.text.clone(),
                buttons: message.buttons.clone(),
                source: message.source(sender.bot_id()),
            }),
            (Sender::Mail(sender), Body::Mail(message)) => Some(Job::Mail {
                sender: sender.clone(),
                letter: message.letter.clone(),
                site: message.site.clone(),
            }),
            _ => None,
        };
        async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            match job {
                Some(Job::Bot {
                    sender,
                    text,
                    buttons,
                    source,
                }) => {
                    let buttons = shown_buttons(&buttons, recipient.locale);
                    let delivery = match recipient.chat_id {
                        Some(chat) => {
                            sender
                                .send(
                                    chat,
                                    &Outgoing {
                                        html: &text,
                                        media: source.media(),
                                        buttons: &buttons,
                                    },
                                )
                                .await
                        }
                        None => Delivery::Failed {
                            error: "У получателя нет чата в Telegram".into(),
                            retry: false,
                        },
                    };
                    Flight {
                        cached: matches!(source, Source::Cached(..)),
                        recipient,
                        answer: Answer::Bot(delivery),
                    }
                }
                Some(Job::Mail {
                    sender,
                    letter,
                    site,
                }) => {
                    let token = recipient.unsubscribe.unwrap_or(PREVIEW_TOKEN);
                    let url = letter::unsubscribe_url(&site, token);
                    let (text, html) = lang::sync_scope(recipient.locale, || {
                        (letter.text(&url), letter.html(&url))
                    });
                    let delivery = match recipient.email.as_deref() {
                        Some(to) => {
                            sender
                                .send(&MailOut {
                                    to,
                                    subject: &letter.subject,
                                    text: &text,
                                    html: &html,
                                    unsubscribe: Some(&url),
                                })
                                .await
                        }
                        None => MailDelivery::Rejected("У получателя нет адреса почты".into()),
                    };
                    Flight {
                        cached: false,
                        recipient,
                        answer: Answer::Mail(delivery),
                    }
                }
                None => Flight {
                    cached: false,
                    recipient,
                    answer: Answer::Mail(MailDelivery::Off("отправитель не совпал".into())),
                },
            }
        }
    }

    async fn settle(
        &self,
        id: i64,
        joined: Result<Flight, JoinError>,
        prepared: &mut Prepared,
        pace: &mut Pace,
    ) -> Result<(), ModuleError> {
        let flight = match joined {
            Ok(flight) => flight,
            Err(err) => {
                tracing::warn!(error = %err, "рассылки: отправка сообщения оборвалась");
                return Ok(());
            }
        };
        match flight.answer {
            Answer::Bot(delivery) => {
                self.settle_bot(
                    id,
                    flight.recipient,
                    delivery,
                    flight.cached,
                    prepared,
                    pace,
                )
                .await
            }
            Answer::Mail(delivery) => {
                self.settle_mail(id, flight.recipient, delivery, prepared, pace)
                    .await
            }
        }
    }

    async fn retry(
        &self,
        id: i64,
        lane: Lane,
        recipient: &Claimed,
        after: Duration,
        error: Option<&str>,
        counted: bool,
    ) -> Result<(), ModuleError> {
        let mut conn = self.ctx.store.conn().await?;
        store::record(
            &mut conn,
            id,
            lane,
            recipient.client_id,
            &Outcome::Retry {
                after_ms: millis(after),
                error,
                counted,
            },
        )
        .await?;
        Ok(())
    }

    async fn settle_bot(
        &self,
        id: i64,
        recipient: Claimed,
        delivery: Delivery,
        cached: bool,
        prepared: &mut Prepared,
        pace: &mut Pace,
    ) -> Result<(), ModuleError> {
        let lane = Lane::Bot;
        let client = recipient.client_id;
        let mut conn = self.ctx.store.conn().await?;
        match delivery {
            Delivery::Sent { file_id } => {
                store::record(&mut conn, id, lane, client, &Outcome::Sent).await?;
                prepared.rejected = 0;
                if let (Some(file_id), Body::Bot(message), Some(bot), false) =
                    (file_id, &mut prepared.body, self.sender.bot_id(), cached)
                    && let Some(image) = message.image.as_ref()
                {
                    store::remember_file(&mut conn, image.id, bot, &file_id).await?;
                    message.cache(bot, file_id);
                }
                if prepared.notice {
                    store::set_notice(&mut conn, id, lane, None).await?;
                    prepared.notice = false;
                }
            }
            Delivery::Blocked(reason) => {
                store::record(&mut conn, id, lane, client, &Outcome::Blocked(&reason)).await?;
            }
            Delivery::Wait(pause) => {
                pace.pause(pause);
                drop(conn);
                self.retry(id, lane, &recipient, pause, None, false).await?;
                let notice = format!(
                    "Telegram попросил подождать {} с: отправка продолжится сама",
                    pause.as_secs().max(1)
                );
                let mut conn = self.ctx.store.conn().await?;
                store::set_notice(&mut conn, id, lane, Some(&notice)).await?;
                prepared.notice = true;
            }
            Delivery::StaleFile => {
                if let Body::Bot(message) = &mut prepared.body {
                    if let Some(image) = &message.image {
                        store::forget_file(&mut conn, image.id).await?;
                    }
                    message.file = None;
                }
                drop(conn);
                self.retry(id, lane, &recipient, Duration::ZERO, None, false)
                    .await?;
            }
            Delivery::Failed { error, retry: true } if recipient.attempts < self.config.retries => {
                let factor = u32::try_from(recipient.attempts.max(1)).unwrap_or(1);
                drop(conn);
                self.retry(
                    id,
                    lane,
                    &recipient,
                    self.config.retry_pause * factor,
                    Some(&error),
                    true,
                )
                .await?;
            }
            Delivery::Failed { error, retry } => {
                store::record(&mut conn, id, lane, client, &Outcome::Failed(&error)).await?;
                if !retry {
                    prepared.rejected += 1;
                }
                if prepared.rejected >= self.config.reject_limit
                    && store::sent_count(&mut conn, id, lane).await? == 0
                {
                    drop(conn);
                    let mut tx = self.ctx.store.begin().await?;
                    let notice = format!("Остановлена: Telegram не принимает сообщение — {error}");
                    store::stop(&mut tx, id, Some(&notice)).await?;
                    tx.audit()
                        .record(&audit(
                            "broadcast.canceled",
                            None,
                            Some(id),
                            serde_json::json!({ "reason": error }),
                        ))
                        .await?;
                    tx.commit().await?;
                    tracing::warn!(broadcast = id, %error, "рассылка остановлена: Telegram отклоняет сообщение");
                }
            }
            Delivery::NoBot(reason) => {
                pace.pause(self.config.bot_pause);
                drop(conn);
                self.retry(id, lane, &recipient, Duration::ZERO, None, false)
                    .await?;
                let notice = format!(
                    "Бот не подключён: {reason}. Отправка продолжится, когда бот заработает"
                );
                let mut conn = self.ctx.store.conn().await?;
                store::set_notice(&mut conn, id, lane, Some(&notice)).await?;
                prepared.notice = true;
            }
        }
        Ok(())
    }

    async fn settle_mail(
        &self,
        id: i64,
        recipient: Claimed,
        delivery: MailDelivery,
        prepared: &mut Prepared,
        pace: &mut Pace,
    ) -> Result<(), ModuleError> {
        let lane = Lane::Email;
        let client = recipient.client_id;
        match delivery {
            MailDelivery::Sent => {
                let mut conn = self.ctx.store.conn().await?;
                store::record(&mut conn, id, lane, client, &Outcome::Sent).await?;
                if prepared.notice {
                    store::set_notice(&mut conn, id, lane, None).await?;
                    prepared.notice = false;
                }
            }
            MailDelivery::Rejected(reason) => {
                let mut conn = self.ctx.store.conn().await?;
                store::record(&mut conn, id, lane, client, &Outcome::Failed(&reason)).await?;
            }
            MailDelivery::Wait(pause) => {
                pace.pause(pause);
                self.retry(id, lane, &recipient, pause, None, false).await?;
                let notice = format!(
                    "Почтовый сервис попросил подождать {} с: отправка продолжится сама",
                    pause.as_secs().max(1)
                );
                let mut conn = self.ctx.store.conn().await?;
                store::set_notice(&mut conn, id, lane, Some(&notice)).await?;
                prepared.notice = true;
            }
            MailDelivery::Failed { error, retry: true }
                if recipient.attempts < self.config.retries =>
            {
                let factor = u32::try_from(recipient.attempts.max(1)).unwrap_or(1);
                self.retry(
                    id,
                    lane,
                    &recipient,
                    self.config.retry_pause * factor,
                    Some(&error),
                    true,
                )
                .await?;
            }
            MailDelivery::Failed { error, .. } => {
                let mut conn = self.ctx.store.conn().await?;
                store::record(&mut conn, id, lane, client, &Outcome::Failed(&error)).await?;
            }
            MailDelivery::Off(reason) => {
                pace.pause(self.config.bot_pause);
                self.retry(id, lane, &recipient, Duration::ZERO, None, false)
                    .await?;
                let notice =
                    format!("Почта не отправляет: {reason}. Письма уйдут, когда почта заработает");
                let mut conn = self.ctx.store.conn().await?;
                store::set_notice(&mut conn, id, lane, Some(&notice)).await?;
                prepared.notice = true;
            }
        }
        Ok(())
    }

    async fn after(&self, id: i64, stopping: bool) -> Result<Duration, ModuleError> {
        let mut conn = self.ctx.store.conn().await?;
        if store::complete(&mut conn, id).await? {
            tracing::info!(broadcast = id, "рассылка отправлена");
            return Ok(Duration::ZERO);
        }
        if stopping {
            return Ok(self.config.idle);
        }
        if store::status(&mut conn, id).await? != Some(Status::Sending) {
            return Ok(Duration::ZERO);
        }
        if store::abandon_flights(&mut conn, self.lane(), Some((id, STALE_FLIGHT_SECS))).await? > 0
        {
            return Ok(Duration::ZERO);
        }
        let wait = store::next_retry_ms(&mut conn, id, self.lane())
            .await?
            .map_or(self.config.idle, |ms| {
                Duration::from_millis(u64::try_from(ms).unwrap_or(0))
            });
        Ok(wait.clamp(Duration::from_millis(10), self.config.idle))
    }
}

fn shown_buttons(buttons: &[OutButton], locale: Locale) -> Vec<OutButton> {
    buttons
        .iter()
        .map(|button| {
            let text = match (locale, button.text.as_str()) {
                (Locale::En, "Участвовать") => "Join".to_string(),
                (Locale::En, "Открыть конкурс") => "Open the giveaway".to_string(),
                (Locale::En, "Итоги конкурса") => "Giveaway results".to_string(),
                _ => button.text.clone(),
            };
            OutButton {
                text,
                ..button.clone()
            }
        })
        .collect()
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod pace_tests {
    use super::*;

    fn config(per_second: u32, gap_ms: u64) -> EngineConfig {
        EngineConfig {
            per_second,
            chat_gap: Duration::from_millis(gap_ms),
            ..EngineConfig::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn темп_не_выше_заданного_в_секунду() {
        let mut pace = Pace::new(&config(25, 1000));
        let start = Instant::now();
        let mut sent = 0;
        while start.elapsed() < Duration::from_secs(1) {
            let wait = pace.wait();
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
                continue;
            }
            assert_eq!(pace.take(), Duration::ZERO);
            sent += 1;
        }
        assert!((24..=26).contains(&sent), "за секунду ушло {sent}");
    }

    #[tokio::test(start_paused = true)]
    async fn подряд_взятые_места_раздвигаются_по_темпу() {
        let mut pace = Pace::new(&config(25, 1000));
        assert_eq!(pace.take(), Duration::ZERO);
        assert_eq!(pace.take(), Duration::from_millis(40));
        assert_eq!(pace.take(), Duration::from_millis(80));
        pace.pause(Duration::from_secs(2));
        assert_eq!(pace.take(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn один_чат_не_чаще_раза_в_секунду() {
        let mut pace = Pace::new(&config(25, 1000));
        assert_eq!(pace.chat(7), Duration::ZERO);
        assert_eq!(pace.chat(8), Duration::ZERO);
        assert_eq!(pace.chat(7), Duration::from_secs(1));
        assert_eq!(pace.chat(7), Duration::from_secs(2));
        tokio::time::advance(Duration::from_millis(2500)).await;
        assert_eq!(pace.chat(7), Duration::from_millis(500));
        assert_eq!(pace.chat(8), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn пауза_по_просьбе_телеграма_держит_всех() {
        let mut pace = Pace::new(&config(25, 1000));
        pace.pause(Duration::from_secs(7));
        assert_eq!(pace.wait(), Duration::from_secs(7));
        pace.pause(Duration::from_secs(2));
        assert_eq!(pace.wait(), Duration::from_secs(7));
        tokio::time::advance(Duration::from_secs(7)).await;
        assert_eq!(pace.wait(), Duration::ZERO);
    }

    #[test]
    fn почте_свой_темп_и_без_зазора_по_чату() {
        let mail = EngineConfig::mail();
        assert_eq!(mail.per_second, 2);
        assert!(mail.chat_gap.is_zero());
        assert!(mail.grace > Duration::from_secs(20));
    }
}
