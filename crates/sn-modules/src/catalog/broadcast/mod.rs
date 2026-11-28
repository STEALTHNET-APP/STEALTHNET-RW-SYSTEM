mod auto;
mod auto_store;
#[cfg(test)]
mod auto_tests;
mod engine;
mod kept;
mod letter;
mod page;
mod presets;
mod routes;
mod store;
#[cfg(test)]
mod tests;

pub use routes::TEST_LETTERS;

use crate::catalog::{contests, options, promo, referral};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ScheduledTask};
use crate::sender::{
    BotSender, Delivery, MailDelivery, MailOut, MailSender, MailSite, MediaFile, OutButton,
    Outgoing,
};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Duration as Span, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::bot_content::{
    ButtonStyle, CAPTION_MAX, EMOJI_ID_MAX, TEXT_MAX, normalize_markup, visible_len,
};
use sn_domain::client::Locale;
use sn_domain::format::plural_ru;
use sn_domain::ids::{AdminId, CategoryId, ClientId, TariffId};
use sn_domain::image::{self, ImageKind};
use sn_domain::module::ModuleId;
use sn_domain::money::Currency;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::video::{self, VideoInfo, VideoProblem};
use sn_store::{ActorKind, AuditEntry, ClientConditions, ClientSegment, Db, StoreError};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub use auto::{
    AUTO_EVERY_SECS, AUTO_TASK, AutoReach, CATCH_UP_HOURS, CheckReport, Delay, Localized,
    ReachQuery, Rule, RuleDraft, RuleItem, RuleStats, Trigger, Unit, check as check_rules,
    create_rule, delete_rule, english_on, rule, rule_reach, rules, switch_rule, update_rule,
};
pub use auto_store::{contest_batch, contest_progress, open_batch, queue_bot, stop_contest};
pub use engine::{Engine, EngineConfig, Sender};
pub use letter::{Letter, body_html, body_text};
pub use presets::{PRESETS_KEY, Seeded, enable as enable_presets, seed as seed_presets};
pub use store::{
    Broadcast, ChannelProgress, FailedEmail, ImageMeta, ListItem, Problem, Progress, Reach,
    Subscriber,
};

pub const CLEANUP_TASK: &str = "cleanup";
pub const BUTTONS_MAX: usize = 6;
pub const BUTTON_TEXT_MAX: usize = 64;
pub const URL_MAX: usize = 512;
pub const RAW_MAX: usize = 16_384;
pub const DAYS_MAX: u16 = 90;
pub const SCHEDULE_DAYS_MAX: i64 = 90;
pub const IMAGE_MAX_BYTES: usize = 10_000_000;
pub const VIDEO_MAX_BYTES: usize = 50_000_000;
pub const MEDIA_PART_BYTES: u64 = 2 * 1024 * 1024;
pub const IMAGE_MIN_SIDE: u32 = 100;
pub const IMAGE_MAX_SIDES: u32 = 10_000;
pub const IMAGE_MAX_RATIO: u32 = 20;
pub const CALLBACK_PREFIX: &str = "n:";
pub const AUDIT_ENTITY: &str = "broadcast";
pub const SUBJECT_MAX: usize = 150;
pub const PREVIEW_TOKEN: Uuid = Uuid::nil();
const ORPHAN_AFTER_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Draft,
    Scheduled,
    Sending,
    Done,
    Canceled,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Scheduled => "scheduled",
            Status::Sending => "sending",
            Status::Done => "done",
            Status::Canceled => "canceled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Status::Draft,
            Status::Scheduled,
            Status::Sending,
            Status::Done,
            Status::Canceled,
        ]
        .into_iter()
        .find(|s| s.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Bot,
    Email,
    Both,
}

impl Channel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Channel::Bot => "bot",
            Channel::Email => "email",
            Channel::Both => "both",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Channel::Bot, Channel::Email, Channel::Both]
            .into_iter()
            .find(|c| c.as_str() == value)
    }

    pub const fn title(self) -> &'static str {
        match self {
            Channel::Bot => "В бот",
            Channel::Email => "На почту",
            Channel::Both => "В бот и на почту",
        }
    }

    pub fn lanes(self) -> Vec<Lane> {
        match self {
            Channel::Bot => vec![Lane::Bot],
            Channel::Email => vec![Lane::Email],
            Channel::Both => vec![Lane::Bot, Lane::Email],
        }
    }

    pub fn has(self, lane: Lane) -> bool {
        self.lanes().contains(&lane)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    Photo,
    Video,
}

impl MediaKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            MediaKind::Photo => "photo",
            MediaKind::Video => "video",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [MediaKind::Photo, MediaKind::Video]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }

    const fn with(self) -> &'static str {
        match self {
            MediaKind::Photo => "С картинкой",
            MediaKind::Video => "С видео",
        }
    }

    const fn object(self) -> &'static str {
        match self {
            MediaKind::Photo => "картинку",
            MediaKind::Video => "видео",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Bot,
    Email,
}

impl Lane {
    pub const fn as_str(self) -> &'static str {
        match self {
            Lane::Bot => "bot",
            Lane::Email => "email",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Lane::Bot, Lane::Email]
            .into_iter()
            .find(|l| l.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotSection {
    Menu,
    Subscriptions,
    Buy,
    Trial,
    Balance,
    Support,
    Promo,
    Referral,
    Options,
    Contests,
    Documents,
}

impl BotSection {
    pub const ALL: [BotSection; 11] = [
        BotSection::Menu,
        BotSection::Subscriptions,
        BotSection::Buy,
        BotSection::Trial,
        BotSection::Balance,
        BotSection::Support,
        BotSection::Promo,
        BotSection::Referral,
        BotSection::Options,
        BotSection::Contests,
        BotSection::Documents,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            BotSection::Menu => "menu",
            BotSection::Subscriptions => "subscriptions",
            BotSection::Buy => "buy",
            BotSection::Trial => "trial",
            BotSection::Balance => "balance",
            BotSection::Support => "support",
            BotSection::Promo => "promo",
            BotSection::Referral => "referral",
            BotSection::Options => "options",
            BotSection::Contests => "contests",
            BotSection::Documents => "documents",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            BotSection::Menu => "Главное меню",
            BotSection::Subscriptions => "Мои подписки",
            BotSection::Buy => "Купить подписку",
            BotSection::Trial => "Пробный период",
            BotSection::Balance => "Баланс",
            BotSection::Support => "Поддержка",
            BotSection::Promo => "Промокод",
            BotSection::Referral => "Пригласить друга",
            BotSection::Options => "Доп. опции",
            BotSection::Contests => "Конкурсы",
            BotSection::Documents => "Документы",
        }
    }

    pub const fn module(self) -> Option<ModuleId> {
        match self {
            BotSection::Promo => Some(ModuleId::Promo),
            BotSection::Referral => Some(ModuleId::Referral),
            BotSection::Options => Some(ModuleId::Options),
            BotSection::Contests => Some(ModuleId::Contests),
            _ => None,
        }
    }

    pub fn data(self) -> String {
        match self {
            BotSection::Menu => "m".into(),
            BotSection::Subscriptions => "subs".into(),
            BotSection::Buy => "buy".into(),
            BotSection::Trial => "trial".into(),
            BotSection::Balance => "bal".into(),
            BotSection::Support => "sup".into(),
            BotSection::Promo => format!("x:{}", promo::BOT_ENTRY),
            BotSection::Referral => format!("x:{}", referral::BOT_ENTRY),
            BotSection::Options => format!("x:{}", options::BOT_ENTRY),
            BotSection::Contests => format!("x:{}", contests::BOT_ENTRY),
            BotSection::Documents => "docs".into(),
        }
    }

    pub fn callback(self) -> String {
        format!("{CALLBACK_PREFIX}{}", self.data())
    }

    pub const fn web_path(self) -> &'static str {
        match self {
            BotSection::Buy | BotSection::Promo => "/cabinet/pay",
            BotSection::Balance => "/cabinet/topup",
            BotSection::Documents => "/",
            _ => "/cabinet",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Audience {
    All,
    Active,
    Expiring { days: u16 },
    Expired,
    NeverConnected,
    Trial,
    NoSubscription,
    Tariff { tariff_id: TariffId },
    Filter(ClientConditions),
}

pub struct AudienceNames {
    pub tariffs: HashMap<TariffId, String>,
    pub categories: HashMap<CategoryId, String>,
    pub currency: Currency,
}

impl Audience {
    pub fn check(self) -> Result<Self, BroadcastError> {
        match self {
            Audience::Expiring { days } if days == 0 || days > DAYS_MAX => {
                Err(invalid(format!("Кому: число дней от 1 до {DAYS_MAX}")))
            }
            Audience::Tariff { tariff_id } if tariff_id.0 <= 0 => {
                Err(invalid("Кому: выберите тариф"))
            }
            Audience::Filter(conditions) => {
                conditions
                    .check()
                    .map_err(|message| invalid(format!("Кому: {message}")))?;
                if conditions.status == Some(ClientSegment::Blocked) {
                    return Err(invalid(
                        "Кому: заблокированным клиентам рассылки не отправляются",
                    ));
                }
                Ok(Audience::Filter(conditions))
            }
            other => Ok(other),
        }
    }

    pub fn describe(&self, tariff: Option<&str>, names: Option<&AudienceNames>) -> String {
        match self {
            Audience::All => "Все клиенты".into(),
            Audience::Active => "С активной подпиской".into(),
            Audience::Expiring { days } => {
                let n = u64::from(*days);
                format!(
                    "Подписка закончится в течение {n} {}",
                    plural_ru(n, "дня", "дней", "дней")
                )
            }
            Audience::Expired => "Подписка закончилась".into(),
            Audience::NeverConnected => "Не подключались".into(),
            Audience::Trial => "На пробном периоде".into(),
            Audience::NoSubscription => "Ни разу не было подписки".into(),
            Audience::Tariff { .. } => match tariff {
                Some(name) => format!("Тариф «{name}»"),
                None => "Тариф удалён".into(),
            },
            Audience::Filter(conditions) => {
                let fallback = AudienceNames {
                    tariffs: HashMap::new(),
                    categories: HashMap::new(),
                    currency: Currency::default(),
                };
                let names = names.unwrap_or(&fallback);
                let text = conditions
                    .describe(&names.tariffs, &names.categories, names.currency)
                    .join(", ");
                let mut chars = text.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                    None => "Все клиенты".into(),
                }
            }
        }
    }

    pub const fn tariff(&self) -> Option<TariffId> {
        match self {
            Audience::Tariff { tariff_id } => Some(*tariff_id),
            _ => None,
        }
    }

    pub fn uses_money(&self) -> bool {
        matches!(self, Audience::Filter(conditions) if conditions.uses_money())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonSpec {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<BotSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<ButtonStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji_id: Option<String>,
}

impl ButtonSpec {
    pub fn out(&self) -> OutButton {
        OutButton {
            text: self.text.clone(),
            url: self.url.clone(),
            callback: self.section.map(BotSection::callback),
            style: self.style.map(|style| style.as_str().to_string()),
            emoji_id: self.emoji_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Draft {
    #[serde(default)]
    pub channel: Channel,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub image_id: Option<i64>,
    #[serde(default)]
    pub buttons: Vec<ButtonSpec>,
    pub audience: Audience,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub channel: Channel,
    pub subject: Option<String>,
    pub text: String,
    pub image_id: Option<i64>,
    pub buttons: Vec<ButtonSpec>,
    pub audience: Audience,
}

#[derive(Debug, thiserror::Error)]
pub enum BroadcastError {
    #[error("{message}")]
    Rejected { code: &'static str, message: String },
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for BroadcastError {
    fn from(err: StoreError) -> Self {
        BroadcastError::Module(ModuleError::Store(err))
    }
}

fn rejected(code: &'static str, message: impl Into<String>) -> BroadcastError {
    BroadcastError::Rejected {
        code,
        message: message.into(),
    }
}

fn invalid(message: impl Into<String>) -> BroadcastError {
    rejected("broadcast.invalid", message)
}

pub fn clean_text(raw: &str, with_image: bool) -> Result<String, BroadcastError> {
    clean_caption(raw, with_image.then_some(MediaKind::Photo))
}

pub fn clean_caption(raw: &str, media: Option<MediaKind>) -> Result<String, BroadcastError> {
    let raw = raw.replace("\r\n", "\n");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid("Напишите текст сообщения"));
    }
    if trimmed.chars().count() > RAW_MAX {
        return Err(invalid(format!(
            "Текст с разметкой длиннее {RAW_MAX} знаков"
        )));
    }
    let html = normalize_markup(trimmed).map_err(|p| invalid(format!("Текст: {}", p.message())))?;
    if html.contains("href=\"{") {
        return Err(invalid(
            "Текст: у ссылки должен быть адрес вида <a href=\"https://…\">текст</a>",
        ));
    }
    let length = visible_len(&html);
    if let Some(kind) = media
        && length > CAPTION_MAX
    {
        return Err(invalid(format!(
            "{} Telegram показывает до {CAPTION_MAX} знаков, а в тексте {length}. Сократите текст или уберите {}",
            kind.with(),
            kind.object()
        )));
    }
    if length > TEXT_MAX {
        return Err(invalid(format!(
            "Telegram показывает до {TEXT_MAX} знаков, а в тексте {length}"
        )));
    }
    Ok(html)
}

fn host_of(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

pub fn clean_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty()
        || url.len() > URL_MAX
        || url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\\'))
    {
        return None;
    }
    let url = if let Some(name) = url.strip_prefix('@') {
        format!("https://t.me/{name}")
    } else if url.starts_with("t.me/") {
        format!("https://{url}")
    } else {
        url.to_string()
    };
    let lower = url.to_ascii_lowercase();
    let host = if let Some(rest) = lower.strip_prefix("https://") {
        host_of(rest)
    } else if let Some(rest) = lower.strip_prefix("http://") {
        host_of(rest)
    } else if let Some(rest) = lower.strip_prefix("tg://") {
        host_of(rest)
    } else {
        return None;
    };
    let valid = !host.is_empty()
        && !host.starts_with('.')
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    (valid && url.len() <= URL_MAX).then_some(url)
}

pub fn clean_buttons(raw: &[ButtonSpec]) -> Result<Vec<ButtonSpec>, BroadcastError> {
    if raw.len() > BUTTONS_MAX {
        return Err(invalid(format!("Кнопок не больше {BUTTONS_MAX}")));
    }
    raw.iter()
        .enumerate()
        .map(|(index, button)| {
            let n = index + 1;
            let text = button.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                return Err(invalid(format!("Кнопка {n}: напишите надпись")));
            }
            if text.chars().count() > BUTTON_TEXT_MAX {
                return Err(invalid(format!(
                    "Кнопка {n}: надпись длиннее {BUTTON_TEXT_MAX} знаков"
                )));
            }
            let url = button
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty());
            let emoji_id = match button.emoji_id.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(id) if id.len() <= EMOJI_ID_MAX && id.bytes().all(|b| b.is_ascii_digit()) => {
                    Some(id.to_string())
                }
                Some(_) => {
                    return Err(invalid(format!(
                        "Кнопка {n}: код премиум-эмодзи — только цифры, его присылает бот по команде /emoji"
                    )));
                }
            };
            match (url, button.section) {
                (Some(url), None) => {
                    let url = clean_url(url).ok_or_else(|| {
                        invalid(format!(
                            "Кнопка {n}: адрес должен начинаться с https://, http:// или tg://"
                        ))
                    })?;
                    Ok(ButtonSpec {
                        text,
                        url: Some(url),
                        section: None,
                        style: button.style,
                        emoji_id,
                    })
                }
                (None, Some(section)) => Ok(ButtonSpec {
                    text,
                    url: None,
                    section: Some(section),
                    style: button.style,
                    emoji_id,
                }),
                _ => Err(invalid(format!(
                    "Кнопка {n}: укажите адрес ссылки или раздел бота"
                ))),
            }
        })
        .collect()
}

pub fn clean_subject(
    raw: Option<&str>,
    channel: Channel,
) -> Result<Option<String>, BroadcastError> {
    let subject = raw
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !channel.has(Lane::Email) {
        return Ok(Some(subject).filter(|s| !s.is_empty()));
    }
    if subject.is_empty() {
        return Err(invalid("Тема письма: напишите тему"));
    }
    if subject.chars().count() > SUBJECT_MAX {
        return Err(invalid(format!("Тема письма длиннее {SUBJECT_MAX} знаков")));
    }
    if subject.chars().any(char::is_control) {
        return Err(invalid("Тема письма: уберите служебные символы"));
    }
    Ok(Some(subject))
}

pub fn check_image(bytes: &[u8]) -> Result<image::ImageInfo, BroadcastError> {
    let bad = |message: String| rejected("broadcast.image", message);
    if bytes.is_empty() {
        return Err(bad("Файл пустой".into()));
    }
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(bad(format!(
            "Картинка больше {} МБ: Telegram не примет её как фото",
            IMAGE_MAX_BYTES / 1_000_000
        )));
    }
    let info = image::inspect(bytes).ok_or_else(|| {
        bad("Нужна картинка JPEG или PNG: этот файл не открылся как картинка".into())
    })?;
    if !matches!(info.kind, ImageKind::Jpeg | ImageKind::Png) {
        return Err(bad(
            "Telegram показывает в сообщениях только JPEG и PNG".into()
        ));
    }
    let (short, long) = if info.width < info.height {
        (info.width, info.height)
    } else {
        (info.height, info.width)
    };
    if short < IMAGE_MIN_SIDE {
        return Err(bad(format!(
            "Картинка меньше {IMAGE_MIN_SIDE} точек по стороне"
        )));
    }
    if info.width + info.height > IMAGE_MAX_SIDES {
        return Err(bad(format!(
            "Ширина и высота вместе больше {IMAGE_MAX_SIDES} точек"
        )));
    }
    if long > short.saturating_mul(IMAGE_MAX_RATIO) {
        return Err(bad(
            "Картинка слишком вытянутая: одна сторона больше другой в 20 раз".into(),
        ));
    }
    Ok(info)
}

pub fn check_video(bytes: &[u8]) -> Result<VideoInfo, BroadcastError> {
    let bad = |message: String| rejected("broadcast.image", message);
    if bytes.is_empty() {
        return Err(bad("Файл пустой".into()));
    }
    if bytes.len() > VIDEO_MAX_BYTES {
        return Err(bad(format!(
            "Видео больше {} МБ: Telegram не даёт ботам отправлять файлы больше",
            VIDEO_MAX_BYTES / 1_000_000
        )));
    }
    video::inspect(bytes).map_err(|problem| {
        bad(match problem {
            VideoProblem::NotMp4 => "Нужно видео MP4: этот файл не открылся как MP4".into(),
            VideoProblem::QuickTime => {
                "Это ролик MOV, а видео с проигрывателем Telegram показывает из MP4. Сохраните ролик в MP4 и загрузите снова".into()
            }
            VideoProblem::Damaged => {
                "MP4 не открылся: файл повреждён или загружен не до конца".into()
            }
            VideoProblem::NoPicture => "В файле нет изображения, только звук".into(),
        })
    })
}

pub async fn media_of(
    db: &mut impl Db,
    id: Option<i64>,
) -> Result<Option<MediaKind>, BroadcastError> {
    let Some(id) = id else {
        return Ok(None);
    };
    match store::media_kind(db, id).await? {
        Some(kind) => Ok(Some(kind)),
        None => Err(rejected(
            "broadcast.image",
            "Картинка или видео не найдены: загрузите файл заново",
        )),
    }
}

pub fn check_media_channel(
    media: Option<MediaKind>,
    channel: Channel,
) -> Result<(), BroadcastError> {
    if media == Some(MediaKind::Video) && !channel.has(Lane::Bot) {
        return Err(invalid(
            "Видео уходит только в Telegram: для письма загрузите картинку или уберите видео",
        ));
    }
    Ok(())
}

async fn cleaned(db: &mut impl Db, draft: &Draft) -> Result<Cleaned, BroadcastError> {
    let media = media_of(db, draft.image_id).await?;
    check_media_channel(media, draft.channel)?;
    let audience = draft.audience.clone().check()?;
    if let Some(tariff) = audience.tariff()
        && !store::tariff_alive(db, tariff).await?
    {
        return Err(invalid("Кому: такого тарифа нет"));
    }
    Ok(Cleaned {
        channel: draft.channel,
        subject: clean_subject(draft.subject.as_deref(), draft.channel)?,
        text: clean_caption(&draft.text, media)?,
        image_id: draft.image_id,
        buttons: clean_buttons(&draft.buttons)?,
        audience,
    })
}

fn audit(
    action: &str,
    admin: Option<AdminId>,
    id: Option<i64>,
    details: serde_json::Value,
) -> AuditEntry {
    AuditEntry {
        actor_kind: if admin.is_some() {
            ActorKind::Admin
        } else {
            ActorKind::System
        },
        actor_id: admin.map(|a| a.0),
        action: action.into(),
        entity_kind: id.map(|_| AUDIT_ENTITY.into()),
        entity_id: id,
        details,
        ip: None,
    }
}

pub async fn create(
    ctx: &ModuleContext,
    draft: &Draft,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let content = cleaned(&mut tx, draft).await?;
    let id = store::insert(&mut tx, &content, admin).await?;
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn update(
    ctx: &ModuleContext,
    id: i64,
    draft: &Draft,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    if status != Status::Draft {
        return Err(not_draft(status));
    }
    let content = cleaned(&mut tx, draft).await?;
    store::update(&mut tx, id, &content).await?;
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

fn not_draft(status: Status) -> BroadcastError {
    rejected(
        "broadcast.state",
        match status {
            Status::Scheduled => "Рассылка запланирована: отмените отправку, чтобы её изменить",
            Status::Sending => "Рассылка уже идёт",
            Status::Done => "Рассылка уже отправлена",
            Status::Canceled => "Рассылка остановлена",
            Status::Draft => "Рассылка — черновик",
        },
    )
}

pub async fn view(ctx: &ModuleContext, id: i64) -> Result<Broadcast, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::view(&mut conn, id).await?)
}

pub async fn list(
    ctx: &ModuleContext,
    page: PageRequest,
) -> Result<Page<ListItem>, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::list(&mut conn, page).await?)
}

pub async fn delete(ctx: &ModuleContext, id: i64) -> Result<(), BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    if status != Status::Draft {
        return Err(rejected("broadcast.state", "Удалить можно только черновик"));
    }
    store::delete(&mut tx, id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn copy(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let source = store::view(&mut tx, id).await?;
    let content = Cleaned {
        channel: source.channel,
        subject: source.subject,
        text: source.text,
        image_id: source.image.map(|i| i.id),
        buttons: source.buttons,
        audience: source.audience,
    };
    let copied = store::insert(&mut tx, &content, admin).await?;
    let view = store::view(&mut tx, copied).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn reach(ctx: &ModuleContext, audience: Audience) -> Result<Reach, BroadcastError> {
    let audience = audience.check()?;
    let mut conn = ctx.store.conn().await?;
    let now = crate::catalog::db_now(&mut conn).await?;
    Ok(store::reach(&mut conn, &audience, now).await?)
}

pub async fn start(
    ctx: &ModuleContext,
    id: i64,
    at: Option<DateTime<Utc>>,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    if status != Status::Draft {
        return Err(not_draft(status));
    }
    let now = crate::catalog::db_now(&mut tx).await?;
    let current = store::view(&mut tx, id).await?;
    current.audience.clone().check()?;
    if current.channel.has(Lane::Email) {
        mail_site(ctx)?;
        if current.subject.as_deref().is_none_or(str::is_empty) {
            return Err(invalid("Тема письма: напишите тему"));
        }
    }
    match at {
        Some(at) if at > now + Span::seconds(30) => {
            if at > now + Span::days(SCHEDULE_DAYS_MAX) {
                return Err(invalid(format!(
                    "Запланировать можно не дальше чем на {SCHEDULE_DAYS_MAX} дней вперёд"
                )));
            }
            store::schedule(&mut tx, id, at, admin).await?;
            tx.audit()
                .record(&audit(
                    "broadcast.scheduled",
                    admin,
                    Some(id),
                    serde_json::json!({ "send_at": at, "audience": current.audience }),
                ))
                .await?;
        }
        Some(_) => {
            return Err(invalid(
                "Время отправки уже прошло: выберите время позже или отправьте сейчас",
            ));
        }
        None => {
            let total =
                store::launch(&mut tx, id, &current.audience, current.channel, now, admin).await?;
            if total == 0 {
                return Err(rejected(
                    "broadcast.empty",
                    match current.channel {
                        Channel::Bot => {
                            "Получателей нет: в этой аудитории никого с Telegram, кто не остановил бота"
                        }
                        Channel::Email => {
                            "Получателей нет: в этой аудитории ни у кого нет почты или все отписались"
                        }
                        Channel::Both => "Получателей нет: в этой аудитории некому отправить",
                    },
                ));
            }
            tx.audit()
                .record(&audit(
                    "broadcast.started",
                    admin,
                    Some(id),
                    serde_json::json!({ "total": total, "audience": current.audience }),
                ))
                .await?;
        }
    }
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn cancel(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<Broadcast, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let status = store::lock(&mut tx, id).await?;
    let action = match status {
        Status::Scheduled => {
            store::unschedule(&mut tx, id).await?;
            "broadcast.unscheduled"
        }
        Status::Sending => {
            store::stop(&mut tx, id, None).await?;
            "broadcast.canceled"
        }
        other => {
            return Err(rejected(
                "broadcast.state",
                match other {
                    Status::Draft => "Рассылка ещё не отправлялась",
                    Status::Canceled => "Рассылка уже остановлена",
                    _ => "Рассылка уже закончилась",
                },
            ));
        }
    };
    tx.audit()
        .record(&audit(action, admin, Some(id), serde_json::json!({})))
        .await?;
    let view = store::view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub fn mail_site(ctx: &ModuleContext) -> Result<MailSite, BroadcastError> {
    ctx.core
        .mail_sender()
        .ok_or_else(|| "почта не подключена".to_string())
        .and_then(|sender| sender.site())
        .map_err(|reason| rejected("broadcast.mail", format!("Почта не отправляет: {reason}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestResult {
    Sent,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestReport {
    pub result: TestResult,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestTarget {
    Client(ClientId),
    Address(String),
}

pub async fn send_test(
    ctx: &ModuleContext,
    draft: &Draft,
    target: TestTarget,
    admin: Option<AdminId>,
) -> Result<TestReport, BroadcastError> {
    let (report, client) = match target {
        TestTarget::Client(client_id) => (test_bot(ctx, draft, client_id).await?, Some(client_id)),
        TestTarget::Address(address) => (test_mail(ctx, draft, &address).await?, None),
    };
    let mut conn = ctx.store.conn().await?;
    conn.audit()
        .record(&audit(
            "broadcast.test",
            admin,
            None,
            serde_json::json!({ "client_id": client, "result": report.result }),
        ))
        .await?;
    Ok(report)
}

async fn test_bot(
    ctx: &ModuleContext,
    draft: &Draft,
    client_id: ClientId,
) -> Result<TestReport, BroadcastError> {
    let sender = ctx.core.bot_sender().ok_or_else(|| {
        rejected(
            "broadcast.bot",
            "Бот не подключён: проверьте токен в настройках бота",
        )
    })?;
    let (content, chat, image) = {
        let mut conn = ctx.store.conn().await?;
        let content = cleaned(&mut conn, draft).await?;
        let chat = store::chat_of(&mut conn, client_id).await?.ok_or_else(|| {
            rejected(
                "broadcast.no_telegram",
                "У клиента нет Telegram: проверить можно только на клиенте из бота",
            )
        })?;
        let image = match content.image_id {
            Some(id) => Some(store::image(&mut conn, id, false).await?),
            None => None,
        };
        (content, chat, image)
    };
    let buttons: Vec<OutButton> = content.buttons.iter().map(ButtonSpec::out).collect();
    let bot = sender.bot_id();
    let (delivery, image) =
        deliver_once(ctx, &*sender, bot, chat, &content.text, image, &buttons).await?;
    let mut conn = ctx.store.conn().await?;
    if let (
        Delivery::Sent {
            file_id: Some(file_id),
        },
        Some(image),
        Some(bot),
    ) = (&delivery, &image, bot)
    {
        store::remember_file(&mut conn, image.id, bot, file_id).await?;
    }
    match delivery {
        Delivery::Sent { .. } => Ok(TestReport {
            result: TestResult::Sent,
            message: "Сообщение отправлено".into(),
        }),
        Delivery::Blocked(_) => {
            conn.clients().set_bot_unreachable(client_id, true).await?;
            Ok(TestReport {
                result: TestResult::Blocked,
                message: "Клиент остановил бота: сообщение не дошло".into(),
            })
        }
        Delivery::Wait(pause) => Err(rejected(
            "broadcast.telegram",
            format!(
                "Telegram просит подождать {} с и повторить",
                pause.as_secs().max(1)
            ),
        )),
        Delivery::NoBot(reason) => Err(rejected(
            "broadcast.bot",
            format!("Бот не подключён: {reason}"),
        )),
        Delivery::StaleFile => Err(rejected(
            "broadcast.telegram",
            "Telegram не принял сообщение: файл вложения не принят",
        )),
        Delivery::Failed { error, .. } => Err(rejected(
            "broadcast.telegram",
            format!("Telegram не принял сообщение: {error}"),
        )),
    }
}

pub fn letter_for(
    subject: &str,
    text: &str,
    image: Option<Uuid>,
    buttons: &[ButtonSpec],
    site: &MailSite,
) -> Letter {
    Letter::new(
        subject,
        text,
        image.map(|key| letter::image_url(site, key)),
        buttons,
        site,
    )
}

async fn test_mail(
    ctx: &ModuleContext,
    draft: &Draft,
    address: &str,
) -> Result<TestReport, BroadcastError> {
    let address = address.trim().to_lowercase();
    if address.is_empty()
        || address.len() > 254
        || address.chars().any(char::is_whitespace)
        || !address.contains('@')
    {
        return Err(rejected(
            "broadcast.address",
            "Адрес для проверки записан неверно",
        ));
    }
    let site = mail_site(ctx)?;
    let sender: Arc<dyn MailSender> = ctx
        .core
        .mail_sender()
        .ok_or_else(|| rejected("broadcast.mail", "Почта не подключена"))?;
    let mut email_draft = draft.clone();
    email_draft.channel = Channel::Email;
    let (content, key) = {
        let mut conn = ctx.store.conn().await?;
        if media_of(&mut conn, email_draft.image_id).await? == Some(MediaKind::Video) {
            email_draft.image_id = None;
        }
        let content = cleaned(&mut conn, &email_draft).await?;
        let key = match content.image_id {
            Some(id) => Some(store::image_key(&mut conn, id).await?),
            None => None,
        };
        (content, key)
    };
    let subject = content.subject.clone().unwrap_or_default();
    let letter = letter_for(&subject, &content.text, key, &content.buttons, &site);
    let unsubscribe = letter::unsubscribe_url(&site, PREVIEW_TOKEN);
    let delivery = sender
        .send(&MailOut {
            to: &address,
            subject: &subject,
            text: &letter.text(&unsubscribe),
            html: &letter.html(&unsubscribe),
            unsubscribe: Some(&unsubscribe),
        })
        .await;
    match delivery {
        MailDelivery::Sent => Ok(TestReport {
            result: TestResult::Sent,
            message: format!("Письмо отправлено на {address}"),
        }),
        MailDelivery::Rejected(reason) | MailDelivery::Failed { error: reason, .. } => Err(
            rejected("broadcast.mail", format!("Письмо не отправилось: {reason}")),
        ),
        MailDelivery::Off(reason) => Err(rejected(
            "broadcast.mail",
            format!("Почта не отправляет: {reason}"),
        )),
        MailDelivery::Wait(pause) => Err(rejected(
            "broadcast.mail",
            format!(
                "Почтовый сервис просит подождать {} с и повторить",
                pause.as_secs().max(1)
            ),
        )),
    }
}

async fn deliver_once(
    ctx: &ModuleContext,
    sender: &dyn BotSender,
    bot: Option<i64>,
    chat: i64,
    text: &str,
    image: Option<store::ImageData>,
    buttons: &[OutButton],
) -> Result<(Delivery, Option<store::ImageData>), BroadcastError> {
    if let Some(image) = &image
        && let Some(file_id) = image.file_for(bot)
    {
        let delivery = sender
            .send(
                chat,
                &Outgoing {
                    html: text,
                    media: Some(image.media(MediaFile::Cached(file_id))),
                    buttons,
                },
            )
            .await;
        if delivery != Delivery::StaleFile {
            return Ok((delivery, None));
        }
    }
    let mut image = image;
    if let Some(image) = image.as_mut()
        && !image.loaded()
    {
        let mut conn = ctx.store.conn().await?;
        image.bytes = store::image_bytes(&mut conn, image.id).await?;
    }
    let delivery = sender
        .send(
            chat,
            &Outgoing {
                html: text,
                media: image.as_ref().map(store::ImageData::upload),
                buttons,
            },
        )
        .await;
    Ok((delivery, image))
}

pub async fn upload_image(
    ctx: &ModuleContext,
    bytes: &[u8],
    admin: Option<AdminId>,
) -> Result<ImageMeta, BroadcastError> {
    let info = check_image(bytes)?;
    let mut conn = ctx.store.conn().await?;
    Ok(store::insert_image(&mut conn, bytes, info, admin).await?)
}

pub async fn upload_video(
    ctx: &ModuleContext,
    bytes: &[u8],
    admin: Option<AdminId>,
) -> Result<ImageMeta, BroadcastError> {
    let info = check_video(bytes)?;
    let mut conn = ctx.store.conn().await?;
    Ok(store::insert_video(&mut conn, bytes, info, admin).await?)
}

pub async fn upload_media(
    ctx: &ModuleContext,
    bytes: &[u8],
    admin: Option<AdminId>,
) -> Result<ImageMeta, BroadcastError> {
    if video::sniff(bytes) {
        upload_video(ctx, bytes, admin).await
    } else {
        upload_image(ctx, bytes, admin).await
    }
}

pub async fn image_file(ctx: &ModuleContext, id: i64) -> Result<(String, Vec<u8>), BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let image = store::image(&mut conn, id, true).await?;
    Ok((image.content_type, image.bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    From(u64, Option<u64>),
    Last(u64),
}

impl Wanted {
    pub fn parse(header: &str) -> Option<Wanted> {
        let spec = header.trim().strip_prefix("bytes=")?;
        if spec.contains(',') {
            return None;
        }
        let (start, end) = spec.split_once('-')?;
        let (start, end) = (start.trim(), end.trim());
        if start.is_empty() {
            return end.parse().ok().filter(|n| *n > 0).map(Wanted::Last);
        }
        let start = start.parse().ok()?;
        let end = match end {
            "" => None,
            end => Some(end.parse().ok().filter(|end| *end >= start)?),
        };
        Some(Wanted::From(start, end))
    }

    fn bounds(self, total: u64) -> Option<(u64, u64)> {
        let (start, end) = match self {
            Wanted::From(start, end) => (start, end.unwrap_or(u64::MAX)),
            Wanted::Last(count) => (total.saturating_sub(count), u64::MAX),
        };
        if start >= total {
            return None;
        }
        let last = end
            .min(total - 1)
            .min(start.saturating_add(MEDIA_PART_BYTES - 1));
        Some((start, last))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaPart {
    Whole {
        content_type: String,
        kind: MediaKind,
        bytes: Vec<u8>,
    },
    Part {
        content_type: String,
        start: u64,
        end: u64,
        total: u64,
        bytes: Vec<u8>,
    },
    Outside {
        total: u64,
    },
}

pub async fn media_part(
    ctx: &ModuleContext,
    id: i64,
    wanted: Option<Wanted>,
) -> Result<MediaPart, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let Some(wanted) = wanted else {
        let image = store::image(&mut conn, id, true).await?;
        return Ok(MediaPart::Whole {
            kind: image.kind(),
            content_type: image.content_type,
            bytes: image.bytes,
        });
    };
    let stored = store::image_size(&mut conn, id).await?;
    let total = u64::try_from(stored.size).unwrap_or(0);
    let Some((start, end)) = wanted.bounds(total) else {
        return Ok(MediaPart::Outside { total });
    };
    let bytes = store::image_slice(
        &mut conn,
        id,
        i64::try_from(start).unwrap_or(i64::MAX),
        i64::try_from(end - start + 1).unwrap_or(i64::MAX),
    )
    .await?;
    Ok(MediaPart::Part {
        content_type: stored.content_type,
        start,
        end,
        total,
        bytes,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionOption {
    pub key: BotSection,
    pub title: &'static str,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffOption {
    pub id: TariffId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub text: usize,
    pub caption: usize,
    pub raw: usize,
    pub buttons: usize,
    pub button_text: usize,
    pub url: usize,
    pub days: u16,
    pub schedule_days: i64,
    pub image_bytes: usize,
    pub video_bytes: usize,
    pub subject: usize,
}

pub const LIMITS: Limits = Limits {
    text: TEXT_MAX,
    caption: CAPTION_MAX,
    raw: RAW_MAX,
    buttons: BUTTONS_MAX,
    button_text: BUTTON_TEXT_MAX,
    url: URL_MAX,
    days: DAYS_MAX,
    schedule_days: SCHEDULE_DAYS_MAX,
    image_bytes: IMAGE_MAX_BYTES,
    video_bytes: VIDEO_MAX_BYTES,
    subject: SUBJECT_MAX,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MailState {
    pub ready: bool,
    pub reason: Option<String>,
    pub site: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Options {
    pub bot: bool,
    pub bot_username: Option<String>,
    pub mail: MailState,
    pub sections: Vec<SectionOption>,
    pub tariffs: Vec<TariffOption>,
    pub filter: sn_store::FilterOptions,
    pub limits: Limits,
    pub auto: auto::AutoOptions,
}

pub async fn options(ctx: &ModuleContext) -> Result<Options, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let tariffs = store::tariffs(&mut conn).await?;
    let enabled = store::enabled_modules(&mut conn).await?;
    let bot_username = conn
        .settings()
        .get(sn_domain::settings::keys::BOT_USERNAME)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|u| !u.trim().is_empty());
    let bot = ctx
        .core
        .bot_sender()
        .is_some_and(|sender| sender.bot_id().is_some());
    let site = ctx
        .core
        .mail_sender()
        .and_then(|sender| sender.site().err())
        .is_some_and(|reason| reason == crate::sender::NO_SITE_ADDRESS);
    let mail = match mail_site(ctx) {
        Ok(_) => MailState {
            ready: true,
            reason: None,
            site: false,
        },
        Err(err) => MailState {
            ready: false,
            reason: Some(err.to_string()),
            site,
        },
    };
    Ok(Options {
        bot,
        bot_username,
        mail,
        sections: BotSection::ALL
            .into_iter()
            .map(|key| SectionOption {
                key,
                title: key.title(),
                available: key
                    .module()
                    .is_none_or(|m| enabled.iter().any(|e| e == m.as_str())),
            })
            .collect(),
        tariffs,
        filter: sn_store::filter_options(conn.conn()).await?,
        limits: LIMITS,
        auto: auto::options(&mut conn).await?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsubscribe {
    Unknown,
    Preview,
    Asked,
    Done,
    Restored,
}

pub struct UnsubscribeView {
    pub state: Unsubscribe,
    pub email: Option<String>,
    pub brand: String,
    pub locale: Locale,
}

async fn unsubscribe_step(
    ctx: &ModuleContext,
    token: Uuid,
    act: Option<bool>,
) -> Result<UnsubscribeView, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let brand = match mail_site(ctx) {
        Ok(site) => site.brand,
        Err(_) => store::brand(&mut conn).await?,
    };
    if token == PREVIEW_TOKEN {
        return Ok(UnsubscribeView {
            state: Unsubscribe::Preview,
            email: None,
            brand,
            locale: Locale::Ru,
        });
    }
    let Some(who) = store::subscriber(&mut conn, token).await? else {
        return Ok(UnsubscribeView {
            state: Unsubscribe::Unknown,
            email: None,
            brand,
            locale: Locale::Ru,
        });
    };
    let state = match act {
        None if who.unsubscribed => Unsubscribe::Done,
        None => Unsubscribe::Asked,
        Some(true) => {
            if store::unsubscribe(&mut conn, &who).await? {
                conn.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Client,
                        actor_id: Some(who.client_id.0),
                        action: "broadcast.unsubscribed".into(),
                        entity_kind: Some(AUDIT_ENTITY.into()),
                        entity_id: Some(who.broadcast_id),
                        details: serde_json::json!({}),
                        ip: None,
                    })
                    .await?;
            }
            Unsubscribe::Done
        }
        Some(false) => {
            if store::resubscribe(&mut conn, &who.email).await? {
                conn.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Client,
                        actor_id: Some(who.client_id.0),
                        action: "broadcast.resubscribed".into(),
                        entity_kind: Some(AUDIT_ENTITY.into()),
                        entity_id: Some(who.broadcast_id),
                        details: serde_json::json!({}),
                        ip: None,
                    })
                    .await?;
            }
            Unsubscribe::Restored
        }
    };
    let locale = store::client_locale(&mut conn, who.client_id).await?;
    Ok(UnsubscribeView {
        state,
        email: Some(who.email),
        brand,
        locale,
    })
}

pub async fn unsubscribe_view(
    ctx: &ModuleContext,
    token: Uuid,
) -> Result<UnsubscribeView, BroadcastError> {
    unsubscribe_step(ctx, token, None).await
}

pub async fn unsubscribe(
    ctx: &ModuleContext,
    token: Uuid,
) -> Result<UnsubscribeView, BroadcastError> {
    unsubscribe_step(ctx, token, Some(true)).await
}

pub async fn resubscribe(
    ctx: &ModuleContext,
    token: Uuid,
) -> Result<UnsubscribeView, BroadcastError> {
    unsubscribe_step(ctx, token, Some(false)).await
}

pub async fn public_image(
    ctx: &ModuleContext,
    key: Uuid,
) -> Result<Option<(String, axum::body::Bytes)>, BroadcastError> {
    let now = std::time::Instant::now();
    if let Some(kept) = kept::PUBLIC.get(key, now) {
        return Ok(Some(kept));
    }
    let mut conn = ctx.store.conn().await?;
    let Some(image) = store::image_by_key(&mut conn, key).await? else {
        return Ok(None);
    };
    let bytes = axum::body::Bytes::from(image.bytes);
    kept::PUBLIC.put(key, &image.content_type, bytes.clone(), now);
    Ok(Some((image.content_type, bytes)))
}

pub async fn cleanup(ctx: &ModuleContext) -> Result<u64, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::prune_images(&mut conn, ORPHAN_AFTER_HOURS).await?)
}

pub struct BroadcastModule;

#[async_trait]
impl Module for BroadcastModule {
    fn id(&self) -> ModuleId {
        ModuleId::Broadcast
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn public_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::public(ctx))
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![
            ScheduledTask {
                name: CLEANUP_TASK,
                every: Duration::from_secs(6 * 3600),
            },
            ScheduledTask {
                name: AUTO_TASK,
                every: Duration::from_secs(AUTO_EVERY_SECS),
            },
        ]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        match task {
            CLEANUP_TASK => {
                let removed = cleanup(ctx).await?;
                if removed > 0 {
                    tracing::info!(removed, "рассылки: удалены неиспользуемые картинки");
                }
                Ok(())
            }
            AUTO_TASK if ctx.core.core_settings().maintenance.enabled => {
                tracing::debug!("рассылки: идут технические работы, правила ждут");
                Ok(())
            }
            AUTO_TASK => {
                let now = {
                    let mut conn = ctx.store.conn().await?;
                    crate::catalog::db_now(&mut conn).await?
                };
                auto::check(ctx, now).await.map(|_| ())
            }
            other => Err(ModuleError::Invalid(format!("у модуля нет задачи {other}"))),
        }
    }
}
