mod draw;
mod routes;
mod store;
#[cfg(test)]
mod tests;

use crate::catalog::antibot::{self, Guard, Target};
use crate::catalog::broadcast::{self, BotSection, ButtonSpec, Progress, clean_text};
use crate::catalog::prize::{self, Prize, PrizeDraft};
use crate::catalog::promo::PromoKind;
use crate::catalog::{db_now, installation_currency};
use crate::context::{ModuleContext, Notification};
use crate::error::ModuleError;
use crate::module::{BotEntry, Module, ScheduledTask};
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Datelike, Duration, FixedOffset, Offset, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::ledger::LedgerKind;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::{format, lang};
use sn_store::{ActorKind, AuditEntry, ClientPerson, Db, StoreError};
use std::collections::{HashMap, HashSet};

pub use draw::short as short_hash;
pub use store::Counts;

pub const BOT_ENTRY: &str = "contests";
pub const TASK: &str = "run";
pub const TASK_EVERY_SECS: u64 = 60;
pub const ENTITY: &str = "contest";
pub const TITLE_MAX: usize = 100;
pub const DESCRIPTION_MAX: usize = 1000;
pub const WINNERS_MAX: u32 = 100;
pub const TIERS_MAX: usize = 20;
pub const MESSAGES_MAX: usize = 6;
pub const FRIENDS_MAX: u32 = 100;
pub const DURATION_MIN_MINUTES: i64 = 10;
pub const DURATION_MAX_DAYS: i64 = 366;
pub const CATCH_UP_HOURS: i64 = 12;
pub const RECENT_DAYS: i64 = 7;
pub const TEMPLATE_WON: &str = "contest_won";
pub const TEMPLATE_WAITING: &str = "contest_waiting";
pub const ANTIBOT_EXCLUDED: &str = "Антибот: подозрительный аккаунт";
pub const UNAVAILABLE: &str = "Участие для этого аккаунта недоступно";
const NAMED_MAX: usize = 20;
const PRIZES_BATCH: i64 = 50;
const FACTS_CHUNK: usize = 2000;
const LOG_LIMIT: i64 = 100;
const TIERED: &str = "tiered";

const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Scheduled,
    Running,
    Drawing,
    Finished,
    Canceled,
}

impl Status {
    pub fn of(
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
        drawn_at: Option<DateTime<Utc>>,
        canceled_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Self {
        if canceled_at.is_some() {
            Status::Canceled
        } else if drawn_at.is_some() {
            Status::Finished
        } else if now < starts_at {
            Status::Scheduled
        } else if now < ends_at {
            Status::Running
        } else {
            Status::Drawing
        }
    }

    fn of_row(row: &store::ContestRow, now: DateTime<Utc>) -> Self {
        Status::of(
            row.starts_at,
            row.ends_at,
            row.drawn_at,
            row.canceled_at,
            now,
        )
    }

    pub const fn title(self) -> &'static str {
        match self {
            Status::Scheduled => "Запланирован",
            Status::Running => "Идёт",
            Status::Drawing => "Подводим итоги",
            Status::Finished => "Итоги подведены",
            Status::Canceled => "Отменён",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    Outsiders,
    Participants,
    Results,
}

impl Audience {
    pub const fn as_str(self) -> &'static str {
        match self {
            Audience::Outsiders => "outsiders",
            Audience::Participants => "participants",
            Audience::Results => "results",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Audience::Outsiders,
            Audience::Participants,
            Audience::Results,
        ]
        .into_iter()
        .find(|a| a.as_str() == value)
    }

    pub const fn title(self, only_eligible: bool) -> &'static str {
        match (self, only_eligible) {
            (Audience::Outsiders, false) => "Тем, кто ещё не участвует",
            (Audience::Outsiders, true) => "Тем, кто не участвует, но подходит по условиям",
            (Audience::Participants, _) => "Участникам",
            (Audience::Results, _) => "Итоги — участникам без приза",
        }
    }

    fn button(self) -> ButtonSpec {
        let text = match self {
            Audience::Outsiders => "Участвовать",
            Audience::Participants => "Открыть конкурс",
            Audience::Results => "Итоги конкурса",
        };
        ButtonSpec {
            text: text.into(),
            url: None,
            section: Some(BotSection::Contests),
            style: None,
            emoji_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListFilter {
    #[default]
    All,
    Live,
    Done,
}

impl ListFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("all") {
            "all" => Some(ListFilter::All),
            "live" => Some(ListFilter::Live),
            "done" => Some(ListFilter::Done),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EntryFilter {
    #[default]
    All,
    Drawn,
    Excluded,
    Winners,
}

impl EntryFilter {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("all") {
            "all" => Some(EntryFilter::All),
            "drawn" => Some(EntryFilter::Drawn),
            "excluded" => Some(EntryFilter::Excluded),
            "winners" => Some(EntryFilter::Winners),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionRule {
    #[serde(default)]
    pub paid_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurchaseRule {
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub min: Option<Money>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendsRule {
    pub count: u32,
    #[serde(default)]
    pub paid: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase: Option<PurchaseRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friends: Option<FriendsRule>,
}

impl Conditions {
    pub const fn is_open(&self) -> bool {
        self.subscription.is_none() && self.purchase.is_none() && self.friends.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ConditionsDraft {
    #[serde(default)]
    pub subscription: bool,
    #[serde(default)]
    pub paid_only: bool,
    #[serde(default)]
    pub purchase: bool,
    #[serde(default)]
    pub purchase_from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub purchase_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub purchase_min: Option<String>,
    #[serde(default)]
    pub friends: u32,
    #[serde(default)]
    pub friends_paid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MessageDraft {
    #[serde(default)]
    pub id: Option<i64>,
    pub audience: Audience,
    #[serde(default)]
    pub only_eligible: bool,
    #[serde(default)]
    pub send_at: Option<DateTime<Utc>>,
    pub text: String,
    #[serde(default)]
    pub text_en: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrizeTier {
    pub from_place: u32,
    pub to_place: u32,
    pub prize: Prize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrizeConfig {
    Single(Prize),
    Tiered(Vec<PrizeTier>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaceView {
    pub from_place: u32,
    pub to_place: u32,
    pub places: String,
    pub prize: Prize,
    pub label: String,
}

impl PrizeConfig {
    pub const fn is_tiered(&self) -> bool {
        matches!(self, PrizeConfig::Tiered(_))
    }

    pub fn tiers(&self, winners: u32) -> Vec<PrizeTier> {
        match self {
            PrizeConfig::Single(prize) => vec![PrizeTier {
                from_place: 1,
                to_place: winners.max(1),
                prize: prize.clone(),
            }],
            PrizeConfig::Tiered(tiers) => tiers.clone(),
        }
    }

    pub fn prize_for(&self, place: u32) -> Option<&Prize> {
        match self {
            PrizeConfig::Single(prize) => (place > 0).then_some(prize),
            PrizeConfig::Tiered(tiers) => tiers
                .iter()
                .find(|t| (t.from_place..=t.to_place).contains(&place))
                .map(|t| &t.prize),
        }
    }

    pub fn any(&self, check: impl Fn(&Prize) -> bool) -> bool {
        match self {
            PrizeConfig::Single(prize) => check(prize),
            PrizeConfig::Tiered(tiers) => tiers.iter().any(|t| check(&t.prize)),
        }
    }

    fn lines(&self) -> Vec<String> {
        match self {
            PrizeConfig::Single(prize) => vec![prize.label()],
            PrizeConfig::Tiered(tiers) => tiers
                .iter()
                .map(|t| {
                    format!(
                        "{} — {}",
                        places_title(t.from_place, t.to_place),
                        t.prize.label()
                    )
                })
                .collect(),
        }
    }

    pub fn label(&self) -> String {
        self.lines().join("; ")
    }

    pub fn list(&self) -> String {
        self.lines().join("\n")
    }

    pub fn places(&self, winners: u32) -> Vec<PlaceView> {
        self.tiers(winners)
            .into_iter()
            .map(|t| PlaceView {
                places: places_title(t.from_place, t.to_place),
                label: t.prize.label(),
                from_place: t.from_place,
                to_place: t.to_place,
                prize: t.prize,
            })
            .collect()
    }
}

impl Serialize for PrizeConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            PrizeConfig::Single(prize) => prize.serialize(serializer),
            PrizeConfig::Tiered(tiers) => {
                let mut out = serializer.serialize_struct("PrizeConfig", 2)?;
                out.serialize_field("kind", TIERED)?;
                out.serialize_field("tiers", tiers)?;
                out.end()
            }
        }
    }
}

enum Shape<T, P> {
    Single(P),
    Tiered(Vec<T>),
}

fn shape<T, P, E>(value: serde_json::Value) -> Result<Shape<T, P>, E>
where
    T: serde::de::DeserializeOwned,
    P: serde::de::DeserializeOwned,
    E: serde::de::Error,
{
    if value.get("kind").and_then(serde_json::Value::as_str) == Some(TIERED) {
        let tiers = value.get("tiers").cloned().unwrap_or_default();
        return serde_json::from_value(tiers)
            .map(Shape::Tiered)
            .map_err(E::custom);
    }
    serde_json::from_value(value)
        .map(Shape::Single)
        .map_err(E::custom)
}

impl<'de> Deserialize<'de> for PrizeConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match shape::<PrizeTier, Prize, D::Error>(value)? {
            Shape::Single(prize) => PrizeConfig::Single(prize),
            Shape::Tiered(tiers) => PrizeConfig::Tiered(tiers),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PrizeTierDraft {
    pub from_place: u32,
    pub to_place: u32,
    pub prize: PrizeDraft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrizeConfigDraft {
    Single(PrizeDraft),
    Tiered(Vec<PrizeTierDraft>),
}

impl From<PrizeDraft> for PrizeConfigDraft {
    fn from(prize: PrizeDraft) -> Self {
        PrizeConfigDraft::Single(prize)
    }
}

impl<'de> Deserialize<'de> for PrizeConfigDraft {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let found = shape::<PrizeTierDraft, PrizeDraft, D::Error>(value)?;
        Ok(match found {
            Shape::Single(prize) => PrizeConfigDraft::Single(prize),
            Shape::Tiered(tiers) => PrizeConfigDraft::Tiered(tiers),
        })
    }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

pub fn places_title(from: u32, to: u32) -> String {
    let space = format::NBSP;
    match lang::current() {
        Locale::En if from == to => format!("{}{space}place", ordinal(from)),
        Locale::En => format!("{}–{}{space}place", ordinal(from), ordinal(to)),
        Locale::Ru if from == to => format!("{from}{space}место"),
        Locale::Ru => format!("{from}–{to}{space}места"),
    }
}

pub fn place_word(place: u32) -> String {
    match lang::current() {
        Locale::En => ordinal(place),
        Locale::Ru => place.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContestDraft {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub prize: PrizeConfigDraft,
    #[serde(default)]
    pub winners: u32,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: DateTime<Utc>,
    #[serde(default)]
    pub conditions: ConditionsDraft,
    #[serde(default)]
    pub messages: Option<Vec<MessageDraft>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub title: String,
    pub description: Option<String>,
    pub prize: PrizeConfig,
    pub winners: u32,
    pub conditions: Conditions,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageContent {
    pub id: Option<i64>,
    pub audience: Audience,
    pub only_eligible: bool,
    pub send_at: Option<DateTime<Utc>>,
    pub text: String,
    pub text_en: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConditionLine {
    pub key: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Editable {
    pub all: bool,
    pub texts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Planned,
    Queued,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageView {
    pub id: i64,
    pub audience: Audience,
    pub audience_title: &'static str,
    pub only_eligible: bool,
    pub send_at: Option<DateTime<Utc>>,
    pub text: String,
    pub text_en: Option<String>,
    pub state: MessageState,
    pub queued_at: Option<DateTime<Utc>>,
    pub recipients: Option<i32>,
    pub skipped: Option<String>,
    pub progress: Option<Progress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Winner {
    pub place: i32,
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub prize: Option<Prize>,
    pub prize_label: Option<String>,
    pub prize_by: Option<String>,
    pub prize_state: Option<String>,
    pub prize_note: Option<String>,
    pub prize_code: Option<String>,
    pub prize_error: Option<String>,
    pub prize_at: Option<DateTime<Utc>>,
    pub ticket: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogItem {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Contest {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub prize: PrizeConfig,
    pub prize_label: String,
    pub prizes: Vec<PlaceView>,
    pub winners: u32,
    pub conditions: Conditions,
    pub condition_lines: Vec<ConditionLine>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: Status,
    pub status_title: &'static str,
    pub seed_hash: String,
    pub seed: Option<String>,
    pub list_hash: Option<String>,
    pub entries_drawn: Option<i32>,
    pub drawn_at: Option<DateTime<Utc>>,
    pub canceled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub counts: Counts,
    pub messages: Vec<MessageView>,
    pub winners_list: Vec<Winner>,
    pub log: Vec<LogItem>,
    pub editable: Editable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListItem {
    pub id: i64,
    pub title: String,
    pub prize: PrizeConfig,
    pub prize_label: String,
    pub prizes: Vec<PlaceView>,
    pub winners: u32,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: Status,
    pub status_title: &'static str,
    pub counts: Counts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryItem {
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub joined_at: DateTime<Utc>,
    pub excluded: Option<String>,
    pub ticket: Option<String>,
    pub place: Option<i32>,
    pub prize_state: Option<String>,
    pub prize_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub key: &'static str,
    pub text: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientResult {
    pub won: bool,
    pub place: Option<i32>,
    pub prize_label: Option<String>,
    pub excluded: Option<String>,
    pub prize_state: Option<String>,
    pub prize_note: Option<String>,
    pub prize_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientContest {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub prize: PrizeConfig,
    pub prize_label: String,
    pub prizes: Vec<PlaceView>,
    pub winners: u32,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: Status,
    pub entries: i64,
    pub joined: bool,
    pub checks: Vec<Check>,
    pub seed_hash: String,
    pub result: Option<ClientResult>,
}

impl ClientContest {
    pub fn eligible(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinOutcome {
    Joined,
    Already,
    Closed,
    Blocked,
    NotEligible,
    Confirm,
    Denied,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RunReport {
    pub drawn: usize,
    pub prizes: usize,
    pub messages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Options {
    pub currency: Currency,
    pub promo_enabled: bool,
    pub referral_enabled: bool,
    pub broadcast_enabled: bool,
    pub english: bool,
    pub winners_max: u32,
    pub messages_max: usize,
    pub friends_max: u32,
    pub title_max: usize,
    pub description_max: usize,
}

fn invalid(message: impl Into<String>) -> ModuleError {
    ModuleError::Invalid(message.into())
}

fn decode(what: &str) -> ModuleError {
    ModuleError::Store(StoreError::Decode(format!("конкурс: {what}")))
}

fn parse_prize(row: &store::ContestRow) -> Result<PrizeConfig, ModuleError> {
    serde_json::from_value(row.prize.clone()).map_err(|_| decode("приз"))
}

fn parse_conditions(row: &store::ContestRow) -> Result<Conditions, ModuleError> {
    serde_json::from_value(row.conditions.clone()).map_err(|_| decode("условия"))
}

fn winners_of(row: &store::ContestRow) -> u32 {
    u32::try_from(row.winners).unwrap_or(1)
}

pub fn offset_of(minutes: i64) -> FixedOffset {
    let seconds = i32::try_from(minutes.clamp(-1439, 1439) * 60).unwrap_or(0);
    FixedOffset::east_opt(seconds).unwrap_or(Utc.fix())
}

const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn day(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    let months = match lang::current() {
        Locale::Ru => MONTHS,
        Locale::En => MONTHS_EN,
    };
    format!(
        "{}{}{}",
        local.day(),
        format::NBSP,
        months[local.month0() as usize]
    )
}

pub fn when(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    format!(
        "{} {} {:02}:{:02}",
        day(at, offset),
        lang::pick("в", "at"),
        local.hour(),
        local.minute()
    )
}

fn named(ids: &[i64]) -> String {
    let shown: Vec<String> = ids
        .iter()
        .take(NAMED_MAX)
        .map(|id| format!("#{id}"))
        .collect();
    let word = if ids.len() == 1 {
        "клиент"
    } else {
        "клиенты"
    };
    match ids.len().saturating_sub(NAMED_MAX) {
        0 => format!("{word} {}", shown.join(", ")),
        rest => format!("{word} {} и ещё {rest}", shown.join(", ")),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn purchase_window(
    rule: &PurchaseRule,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        rule.from.unwrap_or(starts_at),
        rule.until.unwrap_or(ends_at),
    )
}

fn subscription_text(rule: SubscriptionRule) -> String {
    if rule.paid_only {
        lang::pick(
            "Действующая купленная или подаренная подписка",
            "An active subscription that was bought or received as a gift",
        )
        .into()
    } else {
        lang::pick("Действующая подписка", "An active subscription").into()
    }
}

fn purchase_text(
    rule: &PurchaseRule,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    offset: FixedOffset,
) -> String {
    let locale = lang::current();
    let english = locale == Locale::En;
    let base = if rule.from.is_none() && rule.until.is_none() {
        lang::pick(
            "Оплата подписки или опций во время конкурса",
            "A payment for a subscription or add-ons during the giveaway",
        )
        .to_string()
    } else {
        let (from, until) = purchase_window(rule, starts_at, ends_at);
        let last = until - Duration::seconds(1);
        if day(from, offset) == day(last, offset) && english {
            format!(
                "A payment for a subscription or add-ons on {}",
                day(from, offset)
            )
        } else if day(from, offset) == day(last, offset) {
            format!("Оплата подписки или опций {}", day(from, offset))
        } else if english {
            format!(
                "A payment for a subscription or add-ons from {} to {}",
                day(from, offset),
                day(last, offset)
            )
        } else {
            format!(
                "Оплата подписки или опций с {} по {}",
                day(from, offset),
                day(last, offset)
            )
        }
    };
    match rule.min {
        Some(min) if english => format!("{base}, at least {}", format::money_in(min, locale)),
        Some(min) => format!("{base} от {}", format::money(min)),
        None => base,
    }
}

fn friends_text(rule: FriendsRule, have: Option<i64>) -> String {
    if lang::current() == Locale::En {
        let friends = if rule.count == 1 { "friend" } else { "friends" };
        let mut text = match have {
            Some(have) => format!(
                "Invite {} {friends} during the giveaway: {} of {}",
                rule.count,
                have.min(i64::from(rule.count)),
                rule.count
            ),
            None => format!("Invite {} {friends} during the giveaway", rule.count),
        };
        if rule.paid {
            text.push_str(", each friend has to pay for a subscription");
        }
        return text;
    }
    let word = format::plural_ru(u64::from(rule.count), "друга", "друзей", "друзей");
    let mut text = match have {
        Some(have) => format!(
            "Пригласить {} {word} за время конкурса: {} из {}",
            rule.count,
            have.min(i64::from(rule.count)),
            rule.count
        ),
        None => format!("Пригласить {} {word} за время конкурса", rule.count),
    };
    if rule.paid {
        text.push_str(", друг должен оплатить подписку");
    }
    text
}

pub fn condition_lines(
    conditions: &Conditions,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<ConditionLine> {
    let mut lines = Vec::new();
    if let Some(rule) = conditions.subscription {
        lines.push(ConditionLine {
            key: "subscription",
            text: subscription_text(rule),
        });
    }
    if let Some(rule) = &conditions.purchase {
        lines.push(ConditionLine {
            key: "purchase",
            text: purchase_text(rule, starts_at, ends_at, offset),
        });
    }
    if let Some(rule) = conditions.friends {
        lines.push(ConditionLine {
            key: "friends",
            text: friends_text(rule, None),
        });
    }
    lines
}

fn checks(
    conditions: &Conditions,
    facts: &store::FactsRow,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<Check> {
    let mut out = Vec::new();
    if let Some(rule) = conditions.subscription {
        out.push(Check {
            key: "subscription",
            text: subscription_text(rule),
            ok: facts.subscribed,
        });
    }
    if let Some(rule) = &conditions.purchase {
        out.push(Check {
            key: "purchase",
            text: purchase_text(rule, starts_at, ends_at, offset),
            ok: facts.purchased,
        });
    }
    if let Some(rule) = conditions.friends {
        out.push(Check {
            key: "friends",
            text: friends_text(rule, Some(facts.friends)),
            ok: facts.friends >= i64::from(rule.count),
        });
    }
    out
}

fn facts_query(
    conditions: &Conditions,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    at: DateTime<Utc>,
) -> store::FactsQuery {
    let (purchase_from, purchase_until, purchase_min) = match &conditions.purchase {
        Some(rule) => {
            let (from, until) = purchase_window(rule, starts_at, ends_at);
            (from, until, rule.min.map_or(0, |m| m.minor))
        }
        None => (starts_at, ends_at, 0),
    };
    store::FactsQuery {
        at,
        paid_only: conditions.subscription.is_some_and(|s| s.paid_only),
        purchase_from,
        purchase_until,
        purchase_min,
        friends_from: starts_at,
        friends_until: ends_at.min(at.max(starts_at)),
        friends_paid: conditions.friends.is_some_and(|f| f.paid),
    }
}

async fn facts_for(
    db: &mut impl Db,
    conditions: &Conditions,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    at: DateTime<Utc>,
    ids: &[i64],
) -> Result<HashMap<i64, store::FactsRow>, StoreError> {
    let query = facts_query(conditions, starts_at, ends_at, at);
    let mut out = HashMap::new();
    for chunk in ids.chunks(FACTS_CHUNK) {
        for row in store::facts(db, chunk, &query).await? {
            out.insert(row.id, row);
        }
    }
    if conditions.friends.is_some()
        && let Some(settings) = antibot::guarding(db, Target::Contest).await?
    {
        let mut invites = Vec::new();
        for chunk in ids.chunks(FACTS_CHUNK) {
            invites.extend(store::invites(db, chunk, &query).await?);
        }
        let excluded = antibot::screen_invites(db, &settings, &invites).await?;
        for invite in invites.iter().filter(|i| excluded.contains(&i.invitee)) {
            if let Some(row) = out.get_mut(&invite.referrer.0) {
                row.friends = (row.friends - 1).max(0);
            }
        }
    }
    Ok(out)
}

fn clean_title(raw: &str) -> Result<String, ModuleError> {
    let title = raw.trim();
    if title.is_empty() {
        return Err(invalid("Напишите название конкурса"));
    }
    if title.chars().count() > TITLE_MAX {
        return Err(invalid(format!("Название длиннее {TITLE_MAX} знаков")));
    }
    Ok(title.to_string())
}

fn clean_description(raw: Option<&str>) -> Result<Option<String>, ModuleError> {
    let Some(text) = raw.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if text.chars().count() > DESCRIPTION_MAX {
        return Err(invalid(format!(
            "Описание длиннее {DESCRIPTION_MAX} знаков"
        )));
    }
    Ok(Some(text.to_string()))
}

fn clean_conditions(
    draft: &ConditionsDraft,
    currency: Currency,
) -> Result<Conditions, ModuleError> {
    let subscription = draft.subscription.then_some(SubscriptionRule {
        paid_only: draft.paid_only,
    });
    let purchase = if draft.purchase {
        if let (Some(from), Some(until)) = (draft.purchase_from, draft.purchase_until)
            && until <= from
        {
            return Err(invalid("Покупка: конец периода раньше начала"));
        }
        let min = match draft
            .purchase_min
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            Some(raw) => Some(
                prize::parse_amount(raw, currency)
                    .ok_or_else(|| invalid("Покупка: сумма указана неверно"))?,
            ),
            None => None,
        };
        Some(PurchaseRule {
            from: draft.purchase_from,
            until: draft.purchase_until,
            min,
        })
    } else {
        None
    };
    if draft.friends > FRIENDS_MAX {
        return Err(invalid(format!("Друзей — не больше {FRIENDS_MAX}")));
    }
    let friends = (draft.friends > 0).then_some(FriendsRule {
        count: draft.friends,
        paid: draft.friends_paid,
    });
    Ok(Conditions {
        subscription,
        purchase,
        friends,
    })
}

fn reason(err: &ModuleError) -> String {
    match err {
        ModuleError::Invalid(text) => text.clone(),
        other => other.to_string(),
    }
}

fn clean_prize(
    draft: &PrizeConfigDraft,
    winners: u32,
    currency: Currency,
) -> Result<(PrizeConfig, u32), ModuleError> {
    let drafts = match draft {
        PrizeConfigDraft::Single(prize) => {
            if !(1..=WINNERS_MAX).contains(&winners) {
                return Err(invalid(format!("Победителей — от 1 до {WINNERS_MAX}")));
            }
            let prize = prize::validate(prize, currency, false)?;
            return Ok((PrizeConfig::Single(prize), winners));
        }
        PrizeConfigDraft::Tiered(drafts) => drafts,
    };
    if drafts.is_empty() {
        return Err(invalid("Укажите приз хотя бы для первого места"));
    }
    if drafts.len() > TIERS_MAX {
        return Err(invalid(format!("Разных призов — не больше {TIERS_MAX}")));
    }
    let mut sorted: Vec<&PrizeTierDraft> = drafts.iter().collect();
    sorted.sort_by_key(|t| (t.from_place, t.to_place));
    let mut tiers: Vec<PrizeTier> = Vec::with_capacity(sorted.len());
    let mut next = 1;
    for tier in sorted {
        let (from, to) = (tier.from_place, tier.to_place);
        if from == 0 || to < from {
            return Err(invalid(
                "Места указываются от меньшего к большему, начиная с первого",
            ));
        }
        if to > WINNERS_MAX {
            return Err(invalid(format!("Победителей — не больше {WINNERS_MAX}")));
        }
        if from < next {
            return Err(invalid(format!(
                "{}: эти места уже отданы другому призу",
                places_title(from, to)
            )));
        }
        if from > next {
            return Err(invalid(format!(
                "Нет приза для места {next}: места идут подряд, без пропусков"
            )));
        }
        let prize = prize::validate(&tier.prize, currency, false)
            .map_err(|err| invalid(format!("{}: {}", places_title(from, to), reason(&err))))?;
        tiers.push(PrizeTier {
            from_place: from,
            to_place: to,
            prize,
        });
        next = to + 1;
    }
    let winners = next - 1;
    if tiers.len() == 1 {
        let only = tiers.remove(0);
        return Ok((PrizeConfig::Single(only.prize), winners));
    }
    Ok((PrizeConfig::Tiered(tiers), winners))
}

async fn clean_content(
    db: &mut impl Db,
    draft: &ContestDraft,
    now: DateTime<Utc>,
    keep_start: Option<DateTime<Utc>>,
) -> Result<Content, ModuleError> {
    let currency = installation_currency(db).await?;
    let (prize, winners) = clean_prize(&draft.prize, draft.winners, currency)?;
    if prize.any(|p| matches!(p, Prize::Promo { .. })) && !prize::promo_enabled(db).await? {
        return Err(invalid(
            "Промокод в призе работает, только когда включён модуль «Промокоды»",
        ));
    }
    let starts_at = match keep_start {
        Some(start) => start,
        None => draft.starts_at.filter(|s| *s > now).unwrap_or(now),
    };
    if draft.ends_at < starts_at + Duration::minutes(DURATION_MIN_MINUTES) {
        return Err(invalid(format!(
            "Итоги — не раньше чем через {DURATION_MIN_MINUTES} минут после начала"
        )));
    }
    if draft.ends_at > starts_at + Duration::days(DURATION_MAX_DAYS) {
        return Err(invalid("Конкурс длится не дольше года"));
    }
    Ok(Content {
        title: clean_title(&draft.title)?,
        description: clean_description(draft.description.as_deref())?,
        prize,
        winners,
        conditions: clean_conditions(&draft.conditions, currency)?,
        starts_at,
        ends_at: draft.ends_at,
    })
}

fn clean_messages(
    drafts: &[MessageDraft],
    content: &Content,
    now: DateTime<Utc>,
    queued: &HashSet<i64>,
) -> Result<Vec<MessageContent>, ModuleError> {
    let editable: Vec<&MessageDraft> = drafts
        .iter()
        .filter(|d| d.id.is_none_or(|id| !queued.contains(&id)))
        .collect();
    if editable.len() + queued.len() > MESSAGES_MAX {
        return Err(invalid(format!("Сообщений не больше {MESSAGES_MAX}")));
    }
    if editable
        .iter()
        .filter(|d| d.audience == Audience::Results)
        .count()
        > 1
    {
        return Err(invalid("Сообщение с итогами может быть только одно"));
    }
    editable
        .into_iter()
        .enumerate()
        .map(|(index, draft)| {
            let n = index + 1;
            let text = clean_text(&draft.text, false)
                .map_err(|err| invalid(format!("Сообщение {n}: {err}")))?;
            let text_en = match draft.text_en.as_deref().map(str::trim) {
                Some(raw) if !raw.is_empty() => Some(clean_text(raw, false).map_err(|err| {
                    invalid(format!("Сообщение {n}, текст на английском: {err}"))
                })?),
                _ => None,
            };
            let send_at = match draft.audience {
                Audience::Results => None,
                _ => {
                    let at = draft.send_at.ok_or_else(|| {
                        invalid(format!("Сообщение {n}: укажите, когда отправить"))
                    })?;
                    if at < content.starts_at - Duration::minutes(1) || at >= content.ends_at {
                        return Err(invalid(format!(
                            "Сообщение {n}: время должно быть между началом и итогами"
                        )));
                    }
                    if at < now - Duration::minutes(1) {
                        return Err(invalid(format!("Сообщение {n}: это время уже прошло")));
                    }
                    Some(at.max(content.starts_at))
                }
            };
            Ok(MessageContent {
                id: draft.id,
                audience: draft.audience,
                only_eligible: draft.only_eligible && draft.audience == Audience::Outsiders,
                send_at,
                text,
                text_en,
            })
        })
        .collect()
}

pub fn default_messages(
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    tiered: bool,
) -> Vec<MessageContent> {
    let span = ends_at - starts_at;
    let message =
        |audience: Audience, send_at: Option<DateTime<Utc>>, ru: &str, en: &str| MessageContent {
            id: None,
            audience,
            only_eligible: false,
            send_at,
            text: ru.into(),
            text_en: Some(en.into()),
        };
    let mut out = vec![if tiered {
        message(
            Audience::Outsiders,
            Some(starts_at),
            "<b>{title}</b>\nПризы:\n{prizes}\nПобедителей: {winners}. Итоги — {date}. Нажмите «Участвовать», чтобы попасть в розыгрыш.",
            "<b>{title}</b>\nPrizes:\n{prizes}\nWinners: {winners}. Results on {date}. Tap “Join” to enter the giveaway.",
        )
    } else {
        message(
            Audience::Outsiders,
            Some(starts_at),
            "<b>{title}</b>\nПриз: {prize}. Победителей: {winners}.\nИтоги — {date}. Нажмите «Участвовать», чтобы попасть в розыгрыш.",
            "<b>{title}</b>\nPrize: {prize}. Winners: {winners}.\nResults on {date}. Tap “Join” to enter the giveaway.",
        )
    }];
    if span >= Duration::days(2) {
        out.push(message(
            Audience::Outsiders,
            Some(ends_at - Duration::days(1)),
            "До итогов конкурса «{title}» остался день. Успейте принять участие!",
            "One day left until the results of the “{title}” giveaway. There's still time to join!",
        ));
    }
    if span >= Duration::hours(6) {
        out.push(message(
            Audience::Participants,
            Some(ends_at - Duration::hours(3)),
            "Итоги конкурса «{title}» — {date}. Вы участвуете, удачи!",
            "Results of the “{title}” giveaway on {date}. You're in, good luck!",
        ));
    }
    out.push(message(
        Audience::Results,
        None,
        "Итоги конкурса «{title}» подведены. В этот раз приз достался другим участникам — спасибо, что участвовали!",
        "The results of the “{title}” giveaway are in. This time the prize went to others. Thanks for taking part!",
    ));
    out
}

pub fn render(
    text: &str,
    row_title: &str,
    prize: &PrizeConfig,
    winners: u32,
    ends_at: DateTime<Utc>,
    offset: FixedOffset,
) -> String {
    text.replace("{title}", &escape(row_title))
        .replace("{prizes}", &escape(&prize.list()))
        .replace("{prize}", &escape(&prize.label()))
        .replace("{winners}", &winners.to_string())
        .replace("{date}", &when(ends_at, offset))
}

fn audit(action: &str, admin: Option<AdminId>, id: i64, details: serde_json::Value) -> AuditEntry {
    AuditEntry {
        actor_kind: if admin.is_some() {
            ActorKind::Admin
        } else {
            ActorKind::System
        },
        actor_id: admin.map(|a| a.0),
        action: action.into(),
        entity_kind: Some(ENTITY.into()),
        entity_id: Some(id),
        details,
        ip: None,
    }
}

async fn offset(db: &mut impl Db) -> Result<FixedOffset, StoreError> {
    Ok(offset_of(store::utc_offset_minutes(db).await?))
}

async fn module_on(db: &mut impl Db, module: ModuleId) -> Result<bool, StoreError> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM module_states WHERE module = $1")
            .bind(module.as_str())
            .fetch_optional(db.conn())
            .await?;
    Ok(enabled.unwrap_or(module.info().enabled_by_default))
}

pub async fn options(ctx: &ModuleContext) -> Result<Options, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    Ok(Options {
        currency: installation_currency(&mut conn).await?,
        promo_enabled: module_on(&mut conn, ModuleId::Promo).await?,
        referral_enabled: module_on(&mut conn, ModuleId::Referral).await?,
        broadcast_enabled: module_on(&mut conn, ModuleId::Broadcast).await?,
        english: broadcast::english_on(&mut conn).await?,
        winners_max: WINNERS_MAX,
        messages_max: MESSAGES_MAX,
        friends_max: FRIENDS_MAX,
        title_max: TITLE_MAX,
        description_max: DESCRIPTION_MAX,
    })
}

async fn view(db: &mut impl Db, id: i64, now: DateTime<Utc>) -> Result<Contest, ModuleError> {
    let row = store::row(db, id).await?;
    let prize = parse_prize(&row)?;
    let conditions = parse_conditions(&row)?;
    let offset = offset(db).await?;
    let status = Status::of_row(&row, now);
    let counts = store::counts(db, id).await?;
    let messages = store::messages(db, id).await?;
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    let mut progress = broadcast::contest_progress(db, &ids).await?;
    let messages = messages
        .into_iter()
        .map(|m| {
            let audience = Audience::parse(&m.audience).unwrap_or(Audience::Participants);
            let state = match (&m.queued_at, &m.skipped) {
                (None, _) => MessageState::Planned,
                (Some(_), Some(_)) => MessageState::Skipped,
                (Some(_), None) => MessageState::Queued,
            };
            MessageView {
                id: m.id,
                audience,
                audience_title: audience.title(m.only_eligible),
                only_eligible: m.only_eligible,
                send_at: m.send_at,
                text: m.text,
                text_en: m.text_en,
                state,
                queued_at: m.queued_at,
                recipients: m.recipients,
                skipped: m.skipped,
                progress: progress.remove(&m.id),
            }
        })
        .collect();
    let winner_rows = store::winners(db, id).await?;
    let people = db
        .clients()
        .people(
            &winner_rows
                .iter()
                .map(|w| ClientId(w.client_id))
                .collect::<Vec<_>>(),
        )
        .await?;
    let handers = store::admin_emails(
        db,
        &winner_rows
            .iter()
            .filter_map(|w| w.prize_by)
            .collect::<Vec<_>>(),
    )
    .await?;
    let winners_list = winner_rows
        .into_iter()
        .map(|w| {
            let won = w
                .place
                .and_then(|p| u32::try_from(p).ok())
                .and_then(|p| prize.prize_for(p))
                .cloned();
            Winner {
                place: w.place.unwrap_or_default(),
                client_id: ClientId(w.client_id),
                client: people.get(&ClientId(w.client_id)).cloned(),
                prize_label: won.as_ref().map(Prize::label),
                prize: won,
                prize_by: w.prize_by.and_then(|a| handers.get(&a).cloned()),
                prize_state: w.prize_state,
                prize_note: w.prize_note,
                prize_code: w.prize_code,
                prize_error: w.prize_error,
                prize_at: w.prize_at,
                ticket: w.ticket,
            }
        })
        .collect();
    let log = store::log_rows(db, id, LOG_LIMIT)
        .await?
        .into_iter()
        .map(|l| LogItem {
            id: l.id,
            kind: l.kind,
            text: l.text,
            at: l.created_at,
        })
        .collect();
    Ok(Contest {
        id: row.id,
        prize_label: prize.label(),
        prizes: prize.places(winners_of(&row)),
        winners: winners_of(&row),
        condition_lines: condition_lines(&conditions, row.starts_at, row.ends_at, offset),
        conditions,
        prize,
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        status,
        status_title: status.title(),
        seed: row.drawn_at.map(|_| draw::seed_text(&row.seed)),
        seed_hash: row.seed_hash,
        list_hash: row.list_hash,
        entries_drawn: row.entries_drawn,
        drawn_at: row.drawn_at,
        canceled_at: row.canceled_at,
        created_at: row.created_at,
        counts,
        messages,
        winners_list,
        log,
        editable: Editable {
            all: status == Status::Scheduled,
            texts: matches!(status, Status::Scheduled | Status::Running),
        },
        title: row.title,
        description: row.description,
    })
}

pub async fn contest(ctx: &ModuleContext, id: i64) -> Result<Contest, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    view(&mut conn, id, now).await
}

pub async fn list(
    ctx: &ModuleContext,
    filter: ListFilter,
    page: PageRequest,
) -> Result<Page<ListItem>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    let rows = store::list(&mut conn, filter, page).await?;
    let items = rows
        .items
        .into_iter()
        .map(|r| {
            let prize = parse_prize(&r.contest)?;
            let status = Status::of_row(&r.contest, now);
            Ok(ListItem {
                id: r.contest.id,
                prize_label: prize.label(),
                prizes: prize.places(winners_of(&r.contest)),
                prize,
                winners: winners_of(&r.contest),
                starts_at: r.contest.starts_at,
                ends_at: r.contest.ends_at,
                status,
                status_title: status.title(),
                counts: r.counts,
                title: r.contest.title,
            })
        })
        .collect::<Result<Vec<_>, ModuleError>>()?;
    Ok(Page {
        items,
        next: rows.next,
    })
}

pub async fn entries(
    ctx: &ModuleContext,
    id: i64,
    filter: EntryFilter,
    page: PageRequest,
) -> Result<Page<EntryItem>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    store::row(&mut conn, id).await?;
    let rows = store::entries(&mut conn, id, filter, page).await?;
    let people = conn
        .clients()
        .people(
            &rows
                .items
                .iter()
                .map(|e| ClientId(e.client_id))
                .collect::<Vec<_>>(),
        )
        .await?;
    Ok(Page {
        items: rows
            .items
            .into_iter()
            .map(|e| EntryItem {
                client_id: ClientId(e.client_id),
                client: people.get(&ClientId(e.client_id)).cloned(),
                joined_at: e.joined_at,
                excluded: e.excluded,
                ticket: e.ticket,
                place: e.place,
                prize_state: e.prize_state,
                prize_note: e.prize_note,
            })
            .collect(),
        next: rows.next,
    })
}

pub async fn entries_csv(ctx: &ModuleContext, id: i64) -> Result<String, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let row = store::row(&mut conn, id).await?;
    let rows = store::all_entries(&mut conn, id).await?;
    let config = parse_prize(&row)?;
    let mut out = String::from("client_id,joined_at,in_draw,excluded,ticket,place,prize\n");
    for e in rows {
        let excluded = e.excluded.unwrap_or_default().replace('"', "'");
        let won = e
            .place
            .and_then(|p| u32::try_from(p).ok())
            .and_then(|p| config.prize_for(p))
            .map(|p| p.label().replace('"', "'"))
            .unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},\"{}\",{},{},\"{}\"\n",
            e.client_id,
            e.joined_at.to_rfc3339(),
            if excluded.is_empty() && row.drawn_at.is_some() {
                "yes"
            } else if row.drawn_at.is_some() {
                "no"
            } else {
                ""
            },
            excluded,
            e.ticket.unwrap_or_default(),
            e.place.map(|p| p.to_string()).unwrap_or_default(),
            won,
        ));
    }
    Ok(out)
}

pub async fn create(
    ctx: &ModuleContext,
    draft: &ContestDraft,
    admin: Option<AdminId>,
) -> Result<Contest, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let content = clean_content(&mut tx, draft, now, None).await?;
    let messages = match &draft.messages {
        Some(drafts) => clean_messages(drafts, &content, now, &HashSet::new())?,
        None => default_messages(
            content.starts_at,
            content.ends_at,
            content.prize.is_tiered(),
        ),
    };
    let seed = draw::new_seed();
    let seed_hash = draw::seed_hash(&seed);
    let id = store::insert(&mut tx, &content, &seed, &seed_hash, admin).await?;
    store::replace_messages(&mut tx, id, &messages).await?;
    store::log(
        &mut tx,
        id,
        "created",
        &format!(
            "Конкурс создан. Отпечаток зерна жеребьёвки: {}",
            draw::short(&seed_hash)
        ),
        serde_json::json!({ "seed_hash": seed_hash }),
    )
    .await?;
    tx.audit()
        .record(&audit(
            "contest.created",
            admin,
            id,
            serde_json::json!({ "title": content.title, "prize": &content.prize, "winners": content.winners }),
        ))
        .await?;
    let created = view(&mut tx, id, now).await?;
    tx.commit().await?;
    Ok(created)
}

pub async fn update(
    ctx: &ModuleContext,
    id: i64,
    draft: &ContestDraft,
    admin: Option<AdminId>,
) -> Result<Contest, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::lock(&mut tx, id).await?;
    let status = Status::of_row(&row, now);
    let content = match status {
        Status::Scheduled => clean_content(&mut tx, draft, now, None).await?,
        Status::Running => {
            let mut content = clean_content(&mut tx, draft, now, Some(row.starts_at)).await?;
            let prize = parse_prize(&row)?;
            let conditions = parse_conditions(&row)?;
            if content.prize != prize
                || content.winners != winners_of(&row)
                || content.conditions != conditions
            {
                return Err(invalid(
                    "Конкурс уже идёт: приз, число победителей и условия менять нельзя",
                ));
            }
            if content.ends_at < row.ends_at {
                return Err(invalid(
                    "Конкурс уже идёт: итоги можно только отложить, но не приблизить",
                ));
            }
            content.prize = prize;
            content
        }
        Status::Drawing | Status::Finished | Status::Canceled => {
            return Err(invalid(
                "Конкурс закончился или отменён — изменить его уже нельзя",
            ));
        }
    };
    if let Some(drafts) = &draft.messages {
        let queued: HashSet<i64> = store::messages(&mut tx, id)
            .await?
            .into_iter()
            .filter(|m| m.queued_at.is_some())
            .map(|m| m.id)
            .collect();
        let messages = clean_messages(drafts, &content, now, &queued)?;
        store::replace_messages(&mut tx, id, &messages).await?;
    }
    store::update(&mut tx, id, &content).await?;
    tx.audit()
        .record(&audit(
            "contest.updated",
            admin,
            id,
            serde_json::json!({ "title": content.title, "ends_at": content.ends_at }),
        ))
        .await?;
    let updated = view(&mut tx, id, now).await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn cancel(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<Contest, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::lock(&mut tx, id).await?;
    match Status::of_row(&row, now) {
        Status::Finished => return Err(invalid("Итоги уже подведены — отменять поздно")),
        Status::Canceled => {}
        Status::Scheduled | Status::Running | Status::Drawing => {
            store::cancel(&mut tx, id, now).await?;
            let stopped = broadcast::stop_contest(&mut tx, id, "Конкурс отменён").await?;
            store::log(
                &mut tx,
                id,
                "canceled",
                "Конкурс отменён, жеребьёвки не будет",
                serde_json::json!({ "stopped": stopped }),
            )
            .await?;
            tx.audit()
                .record(&audit(
                    "contest.canceled",
                    admin,
                    id,
                    serde_json::json!({ "stopped": stopped }),
                ))
                .await?;
        }
    }
    let canceled = view(&mut tx, id, now).await?;
    tx.commit().await?;
    Ok(canceled)
}

pub async fn delete(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<(), ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::lock(&mut tx, id).await?;
    let status = Status::of_row(&row, now);
    let sent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM broadcasts b JOIN contest_messages m ON m.id = b.contest_message_id \
         WHERE m.contest_id = $1)",
    )
    .bind(id)
    .fetch_one(tx.conn())
    .await
    .map_err(StoreError::from)?;
    let deletable = !sent
        && match status {
            Status::Scheduled => true,
            Status::Canceled => store::counts(&mut tx, id).await?.entries == 0,
            Status::Running | Status::Drawing | Status::Finished => false,
        };
    if !deletable {
        return Err(invalid(
            "Конкурс уже начался или по нему уходили сообщения — его можно отменить, но не удалить",
        ));
    }
    store::delete(&mut tx, id).await?;
    tx.audit()
        .record(&audit(
            "contest.deleted",
            admin,
            id,
            serde_json::json!({ "title": row.title }),
        ))
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn retry_prize(
    ctx: &ModuleContext,
    id: i64,
    client_id: ClientId,
    admin: Option<AdminId>,
) -> Result<Contest, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::lock(&mut tx, id).await?;
    if row.canceled_at.is_some() {
        return Err(invalid("Конкурс отменён"));
    }
    let entry = store::lock_entry(&mut tx, id, client_id)
        .await?
        .ok_or(ModuleError::NotFound("участник"))?;
    if entry.place.is_none() {
        return Err(invalid("Этот участник не победитель"));
    }
    if entry.prize_state.as_deref() != Some("failed") {
        return Err(invalid("Приз уже выдан или выдаётся"));
    }
    store::set_prize(
        &mut tx,
        id,
        client_id.0,
        &store::PrizeUpdate {
            state: "pending",
            note: None,
            code: None,
            error: None,
            at: None,
        },
    )
    .await?;
    store::log(
        &mut tx,
        id,
        "retry",
        &format!("Приз клиенту #{} будет выдан снова", client_id.0),
        serde_json::json!({ "client_id": client_id.0 }),
    )
    .await?;
    tx.audit()
        .record(&audit(
            "contest.prize_retry",
            admin,
            id,
            serde_json::json!({ "client_id": client_id.0 }),
        ))
        .await?;
    let updated = view(&mut tx, id, now).await?;
    tx.commit().await?;
    Ok(updated)
}

pub async fn hand_prize(
    ctx: &ModuleContext,
    id: i64,
    client_id: ClientId,
    handed: bool,
    admin: Option<AdminId>,
) -> Result<Contest, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::lock(&mut tx, id).await?;
    if row.canceled_at.is_some() {
        return Err(invalid("Конкурс отменён"));
    }
    let entry = store::lock_entry(&mut tx, id, client_id)
        .await?
        .ok_or(ModuleError::NotFound("участник"))?;
    let place = entry
        .place
        .and_then(|p| u32::try_from(p).ok())
        .ok_or_else(|| invalid("Этот участник не победитель"))?;
    let config = parse_prize(&row)?;
    let Some(won) = config.prize_for(place).filter(|p| p.is_custom()) else {
        return Err(invalid("Этот приз бот выдаёт сам, отмечать его не нужно"));
    };
    let state = entry.prize_state.as_deref().unwrap_or_default();
    let (next, by, at, text) = match (handed, state) {
        (true, "manual") => (
            "issued",
            admin,
            Some(now),
            format!(
                "Свой приз «{}» клиенту #{} отмечен выданным",
                won.label(),
                client_id.0
            ),
        ),
        (false, "issued") => (
            "manual",
            None,
            None,
            format!(
                "Снята отметка о выдаче приза «{}» клиенту #{}",
                won.label(),
                client_id.0
            ),
        ),
        (true, "issued") => return Err(invalid("Приз уже отмечен выданным")),
        (false, "manual") => return Err(invalid("Приз ещё не отмечен выданным")),
        _ => return Err(invalid("Приз ещё не готов к выдаче")),
    };
    store::hand(&mut tx, id, client_id.0, next, by, at).await?;
    store::log(
        &mut tx,
        id,
        if handed {
            "prize_handed"
        } else {
            "prize_unhanded"
        },
        &text,
        serde_json::json!({ "client_id": client_id.0, "place": place }),
    )
    .await?;
    tx.audit()
        .record(&audit(
            if handed {
                "contest.prize_handed"
            } else {
                "contest.prize_unhanded"
            },
            admin,
            id,
            serde_json::json!({ "client_id": client_id.0, "place": place, "prize": won.label() }),
        ))
        .await?;
    let updated = view(&mut tx, id, now).await?;
    tx.commit().await?;
    Ok(updated)
}

fn english_months(text: &str) -> String {
    let mut out = text.to_string();
    for (ru, en) in MONTHS.iter().zip(MONTHS_EN) {
        out = out.replace(ru, en);
    }
    out
}

pub fn shown_note(note: &str) -> String {
    if lang::current() == Locale::Ru {
        return note.to_string();
    }
    match note {
        "Деньги уже на балансе." => {
            "The money is already on your balance.".into()
        }
        "Дни уже добавлены к подписке." => {
            "The days are already added to your subscription.".into()
        }
        "Дни добавятся, когда у клиента появится подписка" => {
            "The days will be added as soon as you have a subscription.".into()
        }
        prize::CUSTOM_NOTE => "To get your prize, write to support.".into(),
        other => match other
            .strip_prefix("Промокод действует до ")
            .and_then(|rest| rest.strip_suffix('.'))
        {
            Some(date) => format!("The promo code is valid until {}.", english_months(date)),
            None => other.to_string(),
        },
    }
}

fn shown_exclusion(reason: &str) -> String {
    if lang::current() == Locale::Ru {
        return reason.to_string();
    }
    if reason.starts_with("Не выполнено") {
        return "Some requirements weren't met".into();
    }
    match reason {
        UNAVAILABLE => "Taking part isn't available for this account".into(),
        "Клиент заблокирован" => "The account is blocked".into(),
        "Клиент удалён" => "The account was removed".into(),
        other => other.to_string(),
    }
}

fn client_contest(
    row: store::ContestRow,
    entries: i64,
    entry: Option<&store::EntryRow>,
    facts: Option<&store::FactsRow>,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Result<ClientContest, ModuleError> {
    let prize = parse_prize(&row)?;
    let conditions = parse_conditions(&row)?;
    let status = Status::of_row(&row, now);
    let checks = match facts {
        Some(facts) => checks(&conditions, facts, row.starts_at, row.ends_at, offset),
        None => condition_lines(&conditions, row.starts_at, row.ends_at, offset)
            .into_iter()
            .map(|l| Check {
                key: l.key,
                text: l.text,
                ok: false,
            })
            .collect(),
    };
    let result = match (status, entry) {
        (Status::Finished, Some(entry)) => Some(ClientResult {
            won: entry.place.is_some(),
            place: entry.place,
            prize_label: entry
                .place
                .and_then(|p| u32::try_from(p).ok())
                .and_then(|p| prize.prize_for(p))
                .map(Prize::label),
            excluded: entry.excluded.as_deref().map(|reason| {
                if reason == ANTIBOT_EXCLUDED {
                    shown_exclusion(UNAVAILABLE)
                } else {
                    shown_exclusion(reason)
                }
            }),
            prize_state: entry.prize_state.clone(),
            prize_note: entry.prize_note.as_deref().map(shown_note),
            prize_code: entry.prize_code.clone(),
        }),
        _ => None,
    };
    Ok(ClientContest {
        id: row.id,
        prize_label: prize.label(),
        prizes: prize.places(winners_of(&row)),
        prize,
        winners: winners_of(&row),
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        status,
        entries,
        joined: entry.is_some(),
        checks,
        result,
        seed_hash: row.seed_hash,
        title: row.title,
        description: row.description,
    })
}

async fn client_facts(
    db: &mut impl Db,
    row: &store::ContestRow,
    client_id: ClientId,
    now: DateTime<Utc>,
) -> Result<Option<store::FactsRow>, ModuleError> {
    let conditions = parse_conditions(row)?;
    Ok(facts_for(
        db,
        &conditions,
        row.starts_at,
        row.ends_at,
        now,
        &[client_id.0],
    )
    .await?
    .remove(&client_id.0))
}

pub async fn visible_for(
    ctx: &ModuleContext,
    client_id: ClientId,
) -> Result<Vec<ClientContest>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    let offset = offset(&mut conn).await?;
    let rows = store::open_for(&mut conn, client_id, now, RECENT_DAYS).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (row, joined, entries) in rows {
        let entry = if joined {
            store::entry(&mut conn, row.id, client_id).await?
        } else {
            None
        };
        let facts = if row.drawn_at.is_none() && now < row.ends_at {
            client_facts(&mut conn, &row, client_id, now).await?
        } else {
            None
        };
        out.push(client_contest(
            row,
            entries,
            entry.as_ref(),
            facts.as_ref(),
            now,
            offset,
        )?);
    }
    Ok(out)
}

pub async fn client_view(
    ctx: &ModuleContext,
    id: i64,
    client_id: ClientId,
) -> Result<Option<ClientContest>, ModuleError> {
    Ok(visible_for(ctx, client_id)
        .await?
        .into_iter()
        .find(|c| c.id == id))
}

pub async fn has_running(ctx: &ModuleContext) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let now = db_now(&mut conn).await?;
    Ok(store::running_count(&mut conn, now).await? > 0)
}

pub async fn join_confirmed(
    ctx: &ModuleContext,
    id: i64,
    client_id: ClientId,
    token: &str,
) -> Result<JoinOutcome, ModuleError> {
    antibot::confirm(
        ctx,
        client_id,
        token,
        Target::Contest.as_str(),
        antibot::Answer::button(),
    )
    .await?;
    join(ctx, id, client_id).await
}

pub async fn join(
    ctx: &ModuleContext,
    id: i64,
    client_id: ClientId,
) -> Result<JoinOutcome, ModuleError> {
    let guard = antibot::guard(ctx, client_id, Target::Contest).await?;
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = match store::share(&mut tx, id).await {
        Ok(row) => row,
        Err(StoreError::NotFound(_)) => return Ok(JoinOutcome::Closed),
        Err(err) => return Err(err.into()),
    };
    if Status::of_row(&row, now) != Status::Running {
        return Ok(JoinOutcome::Closed);
    }
    if store::entry(&mut tx, id, client_id).await?.is_some() {
        return Ok(JoinOutcome::Already);
    }
    let Some(facts) = client_facts(&mut tx, &row, client_id, now).await? else {
        return Ok(JoinOutcome::Blocked);
    };
    if facts.blocked {
        return Ok(JoinOutcome::Blocked);
    }
    let details = serde_json::json!({
        "target": Target::Contest.as_str(),
        "contest_id": id,
        "title": row.title,
    });
    let after = |action| {
        antibot::record(
            ctx,
            client_id,
            guard.as_ref(),
            action,
            details.clone(),
            Some(id),
        )
    };
    if Guard::denied(guard.as_ref()) {
        tx.commit().await?;
        after(antibot::Action::ContestDenied).await?;
        return Ok(JoinOutcome::Denied);
    }
    let conditions = parse_conditions(&row)?;
    let offset = offset(&mut tx).await?;
    if !checks(&conditions, &facts, row.starts_at, row.ends_at, offset)
        .iter()
        .all(|c| c.ok)
    {
        tx.commit().await?;
        return Ok(JoinOutcome::NotEligible);
    }
    if Guard::asks(guard.as_ref()) {
        tx.commit().await?;
        after(antibot::Action::ContestAsked).await?;
        return Ok(JoinOutcome::Confirm);
    }
    let added = store::insert_entry(&mut tx, id, client_id).await?;
    tx.commit().await?;
    if !added {
        return Ok(JoinOutcome::Already);
    }
    if Guard::noted(guard.as_ref()) {
        after(antibot::Action::ContestNoted).await?;
    }
    Ok(JoinOutcome::Joined)
}

pub async fn run(ctx: &ModuleContext) -> Result<RunReport, ModuleError> {
    let now = {
        let mut conn = ctx.store.conn().await?;
        db_now(&mut conn).await?
    };
    let mut report = RunReport::default();
    let due = {
        let mut conn = ctx.store.conn().await?;
        store::due_draws(&mut conn, now).await?
    };
    for id in due {
        if draw_one(ctx, id).await? {
            report.drawn += 1;
        }
    }
    report.prizes = issue_prizes(ctx).await?;
    report.messages = queue_messages(ctx).await?;
    Ok(report)
}

async fn draw_one(ctx: &ModuleContext, id: i64) -> Result<bool, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::lock(&mut tx, id).await?;
    if row.drawn_at.is_some() || row.canceled_at.is_some() || row.ends_at > now {
        tx.rollback().await?;
        return Ok(false);
    }
    let conditions = parse_conditions(&row)?;
    let offset = offset(&mut tx).await?;
    let ids = store::entry_ids(&mut tx, id).await?;
    let facts = facts_for(
        &mut tx,
        &conditions,
        row.starts_at,
        row.ends_at,
        row.ends_at,
        &ids,
    )
    .await?;
    let suspects = antibot::blocked_among(&mut tx, Target::Contest, &ids).await?;
    let mut excluded: Vec<(i64, String)> = Vec::new();
    let mut eligible: Vec<i64> = Vec::new();
    let mut caught: Vec<i64> = Vec::new();
    for client in &ids {
        match facts.get(client) {
            None => excluded.push((*client, "Клиент удалён".into())),
            Some(f) if f.blocked => excluded.push((*client, "Клиент заблокирован".into())),
            Some(_) if suspects.contains_key(&ClientId(*client)) => {
                excluded.push((*client, ANTIBOT_EXCLUDED.into()));
                caught.push(*client);
            }
            Some(f) => {
                let failed: Vec<String> =
                    checks(&conditions, f, row.starts_at, row.ends_at, offset)
                        .into_iter()
                        .filter(|c| !c.ok)
                        .map(|c| c.text)
                        .collect();
                if failed.is_empty() {
                    eligible.push(*client);
                } else {
                    excluded.push((*client, format!("Не выполнено: {}", failed.join("; "))));
                }
            }
        }
    }
    store::exclude(&mut tx, id, &excluded).await?;
    if !caught.is_empty() {
        for client in &caught {
            antibot::note(
                &mut tx,
                ClientId(*client),
                antibot::Action::ContestExcluded,
                suspects
                    .get(&ClientId(*client))
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                serde_json::json!({
                    "target": Target::Contest.as_str(),
                    "contest_id": id,
                    "title": row.title,
                }),
                Some(id),
            )
            .await?;
        }
        store::log(
            &mut tx,
            id,
            "antibot",
            &format!(
                "Не прошли проверку антибота — {}: {}",
                caught.len(),
                named(&caught)
            ),
            serde_json::json!({ "clients": caught }),
        )
        .await?;
    }
    let drawn = draw::tickets(&row.seed, &eligible);
    let places = (winners_of(&row) as usize).min(drawn.len());
    let rows: Vec<(i64, String, Option<i32>)> = drawn
        .iter()
        .enumerate()
        .map(|(i, d)| {
            (
                d.client_id,
                d.ticket.clone(),
                (i < places).then(|| i32::try_from(i + 1).unwrap_or(i32::MAX)),
            )
        })
        .collect();
    store::set_tickets(&mut tx, id, &rows).await?;
    let list_hash = draw::list_hash(&eligible);
    let total = i64::try_from(eligible.len()).unwrap_or(i64::MAX);
    store::set_drawn(&mut tx, id, &list_hash, total, now).await?;
    let seed = draw::seed_text(&row.seed);
    let text = if eligible.is_empty() {
        format!(
            "Жеребьёвка: участников в розыгрыше нет{}",
            if excluded.is_empty() {
                String::new()
            } else {
                format!(", не прошли проверку — {}", excluded.len())
            }
        )
    } else {
        format!(
            "Жеребьёвка: в розыгрыше {}, выбрано победителей — {}{}. Зерно {}, отпечаток списка {}",
            eligible.len(),
            places,
            if excluded.is_empty() {
                String::new()
            } else {
                format!(", не прошли проверку — {}", excluded.len())
            },
            draw::short(&seed),
            draw::short(&list_hash)
        )
    };
    store::log(
        &mut tx,
        id,
        "draw",
        &text,
        serde_json::json!({
            "seed": seed,
            "seed_hash": row.seed_hash,
            "list_hash": list_hash,
            "entries": eligible.len(),
            "excluded": excluded.len(),
            "winners": drawn.iter().take(places).map(|d| d.client_id).collect::<Vec<_>>(),
        }),
    )
    .await?;
    tx.audit()
        .record(&audit(
            "contest.drawn",
            None,
            id,
            serde_json::json!({ "entries": eligible.len(), "winners": places }),
        ))
        .await?;
    tx.commit().await?;
    tracing::info!(
        contest = id,
        entries = eligible.len(),
        winners = places,
        "конкурс: жеребьёвка проведена"
    );
    Ok(true)
}

async fn issue_prizes(ctx: &ModuleContext) -> Result<usize, ModuleError> {
    let due = {
        let mut conn = ctx.store.conn().await?;
        store::prizes_due(&mut conn, PRIZES_BATCH).await?
    };
    let mut done = 0;
    for (contest_id, client_id) in due {
        match issue_one(ctx, contest_id, ClientId(client_id)).await {
            Ok(true) => done += 1,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(contest = contest_id, client = client_id, error = %err, "конкурс: приз не выдан, повторим");
            }
        }
    }
    Ok(done)
}

async fn notify_winner(
    ctx: &ModuleContext,
    contest_id: i64,
    client_id: ClientId,
    template: &str,
    params: serde_json::Value,
    mark: &str,
) {
    match ctx
        .core
        .notify(
            client_id,
            Notification {
                template: template.into(),
                params,
            },
        )
        .await
    {
        Ok(()) => {
            let marked = match ctx.store.conn().await {
                Ok(mut conn) => store::set_notified(&mut conn, contest_id, client_id.0, mark)
                    .await
                    .map_err(ModuleError::from),
                Err(err) => Err(err.into()),
            };
            if let Err(err) = marked {
                tracing::warn!(contest = contest_id, client = %client_id, error = %err, "конкурс: отметка об уведомлении не сохранена");
            }
        }
        Err(err) => {
            tracing::warn!(contest = contest_id, client = %client_id, error = %err, "конкурс: уведомление победителю не поставлено");
        }
    }
}

async fn issue_one(
    ctx: &ModuleContext,
    contest_id: i64,
    client_id: ClientId,
) -> Result<bool, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let row = store::share(&mut tx, contest_id).await?;
    let Some(entry) = store::lock_entry(&mut tx, contest_id, client_id).await? else {
        return Ok(false);
    };
    let state = entry.prize_state.as_deref().unwrap_or_default();
    if row.canceled_at.is_some() || !matches!(state, "pending" | "waiting") {
        return Ok(false);
    }
    let config = parse_prize(&row)?;
    let place = entry
        .place
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or_default();
    let Some(prize) = config.prize_for(place).cloned() else {
        store::set_prize(
            &mut tx,
            contest_id,
            client_id.0,
            &store::PrizeUpdate {
                state: "failed",
                note: None,
                code: None,
                error: Some("Для этого места в конкурсе нет приза"),
                at: None,
            },
        )
        .await?;
        tx.commit().await?;
        return Ok(false);
    };
    let offset = offset(&mut tx).await?;
    let blocked: bool = sqlx::query_scalar(
        "SELECT status <> 'active' OR deleted_at IS NOT NULL FROM clients WHERE id = $1",
    )
    .bind(client_id.0)
    .fetch_optional(tx.conn())
    .await
    .map_err(StoreError::from)?
    .unwrap_or(true);
    let base = serde_json::json!({
        "title": row.title,
        "prize": prize.label(),
        "prize_data": &prize,
        "contest_id": contest_id,
        "place": place,
        "tiered": config.is_tiered(),
    });
    if blocked {
        store::set_prize(
            &mut tx,
            contest_id,
            client_id.0,
            &store::PrizeUpdate {
                state: "failed",
                note: None,
                code: None,
                error: Some("Клиент заблокирован или удалён"),
                at: None,
            },
        )
        .await?;
        store::log(
            &mut tx,
            contest_id,
            "prize_failed",
            &format!(
                "Приз клиенту #{} не выдан: клиент заблокирован или удалён",
                client_id.0
            ),
            serde_json::json!({ "client_id": client_id.0 }),
        )
        .await?;
        tx.commit().await?;
        return Ok(false);
    }
    match &prize {
        Prize::Balance { amount } => {
            let currency = installation_currency(&mut tx).await?;
            prize::check_currency(&prize, currency)?;
            prize::credit(
                &mut tx,
                client_id,
                *amount,
                LedgerKind::Contest,
                format!("contest:{contest_id}:{}", client_id.0),
                format!("Приз конкурса «{}»", row.title),
            )
            .await?;
            let note = "Деньги уже на балансе.".to_string();
            finish(&mut tx, contest_id, client_id, &note, None, now).await?;
            tx.commit().await?;
            notify_winner(
                ctx,
                contest_id,
                client_id,
                TEMPLATE_WON,
                with(base, &note, None),
                "issued",
            )
            .await;
            Ok(true)
        }
        Prize::Promo {
            percent,
            valid_days,
        } => {
            let until = prize::promo_until(now, *valid_days);
            let code = prize::personal_code(
                &mut tx,
                PromoKind::Percent { percent: *percent },
                Some(until),
                &format!("Приз конкурса «{}», клиент #{}", row.title, client_id.0),
                client_id,
            )
            .await?;
            let note = format!("Промокод действует до {}.", day(until, offset));
            finish(&mut tx, contest_id, client_id, &note, Some(&code), now).await?;
            tx.commit().await?;
            notify_winner(
                ctx,
                contest_id,
                client_id,
                TEMPLATE_WON,
                with(base, &note, Some(&code)),
                "issued",
            )
            .await;
            Ok(true)
        }
        Prize::Days { days } => {
            let Some(sub) = prize::live_subscription(&mut tx, client_id, None).await? else {
                if state == "pending" {
                    store::set_prize(
                        &mut tx,
                        contest_id,
                        client_id.0,
                        &store::PrizeUpdate {
                            state: "waiting",
                            note: Some("Дни добавятся, когда у клиента появится подписка"),
                            code: None,
                            error: None,
                            at: None,
                        },
                    )
                    .await?;
                    store::log(
                        &mut tx,
                        contest_id,
                        "prize_waiting",
                        &format!(
                            "Приз клиенту #{} ждёт подписку: дни добавятся, когда она появится",
                            client_id.0
                        ),
                        serde_json::json!({ "client_id": client_id.0 }),
                    )
                    .await?;
                    tx.commit().await?;
                    if entry.notified.is_none() {
                        notify_winner(
                            ctx,
                            contest_id,
                            client_id,
                            TEMPLATE_WAITING,
                            base,
                            "waiting",
                        )
                        .await;
                    }
                } else {
                    tx.rollback().await?;
                }
                return Ok(false);
            };
            store::set_prize(
                &mut tx,
                contest_id,
                client_id.0,
                &store::PrizeUpdate {
                    state: "issuing",
                    note: None,
                    code: None,
                    error: None,
                    at: None,
                },
            )
            .await?;
            tx.commit().await?;
            let applied = ctx
                .core
                .add_days(sub.id, *days, &format!("конкурс «{}»", row.title))
                .await;
            let mut conn = ctx.store.conn().await?;
            match applied {
                Ok(()) => {
                    let note = "Дни уже добавлены к подписке.".to_string();
                    store::set_prize(
                        &mut conn,
                        contest_id,
                        client_id.0,
                        &store::PrizeUpdate {
                            state: "issued",
                            note: Some(&note),
                            code: None,
                            error: None,
                            at: Some(now),
                        },
                    )
                    .await?;
                    store::log(
                        &mut conn,
                        contest_id,
                        "prize_issued",
                        &format!("Приз выдан клиенту #{}: {}", client_id.0, prize.label()),
                        serde_json::json!({ "client_id": client_id.0, "subscription_id": sub.id.0 }),
                    )
                    .await?;
                    drop(conn);
                    notify_winner(
                        ctx,
                        contest_id,
                        client_id,
                        TEMPLATE_WON,
                        with(base, &note, None),
                        "issued",
                    )
                    .await;
                    Ok(true)
                }
                Err(err) => {
                    let error =
                        format!("Дни не добавлены: {err}. Проверьте подписку и выдайте снова");
                    store::set_prize(
                        &mut conn,
                        contest_id,
                        client_id.0,
                        &store::PrizeUpdate {
                            state: "failed",
                            note: None,
                            code: None,
                            error: Some(&error),
                            at: None,
                        },
                    )
                    .await?;
                    store::log(
                        &mut conn,
                        contest_id,
                        "prize_failed",
                        &format!("Приз клиенту #{} не выдан: {err}", client_id.0),
                        serde_json::json!({ "client_id": client_id.0 }),
                    )
                    .await?;
                    Ok(false)
                }
            }
        }
        Prize::Custom { .. } => {
            let note = prize.note().unwrap_or(prize::CUSTOM_NOTE).to_string();
            store::set_prize(
                &mut tx,
                contest_id,
                client_id.0,
                &store::PrizeUpdate {
                    state: "manual",
                    note: Some(&note),
                    code: None,
                    error: None,
                    at: None,
                },
            )
            .await?;
            store::log(
                &mut tx,
                contest_id,
                "prize_manual",
                &format!(
                    "Клиенту #{} положен свой приз «{}» — его выдаёт администратор",
                    client_id.0,
                    prize.label()
                ),
                serde_json::json!({ "client_id": client_id.0, "place": place }),
            )
            .await?;
            tx.commit().await?;
            notify_winner(
                ctx,
                contest_id,
                client_id,
                TEMPLATE_WON,
                with(base, &note, None),
                "issued",
            )
            .await;
            Ok(true)
        }
        Prize::Empty => {
            finish(&mut tx, contest_id, client_id, "Без приза", None, now).await?;
            tx.commit().await?;
            Ok(true)
        }
    }
}

fn with(mut base: serde_json::Value, note: &str, code: Option<&str>) -> serde_json::Value {
    if let Some(map) = base.as_object_mut() {
        map.insert("note".into(), serde_json::Value::String(note.into()));
        if let Some(code) = code {
            map.insert("code".into(), serde_json::Value::String(code.into()));
        }
    }
    base
}

async fn finish(
    db: &mut impl Db,
    contest_id: i64,
    client_id: ClientId,
    note: &str,
    code: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), ModuleError> {
    store::set_prize(
        db,
        contest_id,
        client_id.0,
        &store::PrizeUpdate {
            state: "issued",
            note: Some(note),
            code,
            error: None,
            at: Some(now),
        },
    )
    .await?;
    store::log(
        db,
        contest_id,
        "prize_issued",
        &format!("Приз выдан клиенту #{}: {note}", client_id.0),
        serde_json::json!({ "client_id": client_id.0 }),
    )
    .await?;
    Ok(())
}

async fn queue_messages(ctx: &ModuleContext) -> Result<usize, ModuleError> {
    let due = {
        let mut conn = ctx.store.conn().await?;
        let now = db_now(&mut conn).await?;
        store::due_messages(&mut conn, now).await?
    };
    let mut queued = 0;
    for message in due {
        if queue_one(ctx, message.id).await? {
            queued += 1;
        }
    }
    Ok(queued)
}

async fn queue_one(ctx: &ModuleContext, message_id: i64) -> Result<bool, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let now = db_now(&mut tx).await?;
    let Some(message) = store::lock_message(&mut tx, message_id).await? else {
        return Ok(false);
    };
    let row = store::share(&mut tx, message.contest_id).await?;
    let audience = Audience::parse(&message.audience).unwrap_or(Audience::Participants);
    let skip = if row.canceled_at.is_some() {
        Some("Конкурс отменён".to_string())
    } else if audience == Audience::Results {
        if row.drawn_at.is_none() {
            tx.rollback().await?;
            return Ok(false);
        }
        None
    } else if message.send_at.is_none_or(|at| at > now) {
        tx.rollback().await?;
        return Ok(false);
    } else if now >= row.ends_at {
        Some("Не отправлено: конкурс уже закончился".to_string())
    } else if message
        .send_at
        .is_some_and(|at| now - at > Duration::hours(CATCH_UP_HOURS))
    {
        Some("Не отправлено: время отправки прошло, пока сервер не работал".to_string())
    } else {
        None
    };
    if let Some(reason) = skip {
        store::mark_message(&mut tx, message_id, now, 0, Some(&reason)).await?;
        store::log(
            &mut tx,
            row.id,
            "message_skipped",
            &format!("{}: {reason}", audience.title(message.only_eligible)),
            serde_json::json!({ "message_id": message_id }),
        )
        .await?;
        tx.commit().await?;
        return Ok(false);
    }
    let prize = parse_prize(&row)?;
    let offset = offset(&mut tx).await?;
    let winners = winners_of(&row);
    let rendered = |raw: &str, locale: Locale| {
        lang::sync_scope(locale, || {
            render(raw, &row.title, &prize, winners, row.ends_at, offset)
        })
    };
    let text = match clean_text(&rendered(&message.text, Locale::Ru), false) {
        Ok(text) => text,
        Err(err) => {
            let reason = format!("Не отправлено: {err}");
            store::mark_message(&mut tx, message_id, now, 0, Some(&reason)).await?;
            store::log(
                &mut tx,
                row.id,
                "message_skipped",
                &format!("{}: {reason}", audience.title(message.only_eligible)),
                serde_json::json!({ "message_id": message_id }),
            )
            .await?;
            tx.commit().await?;
            return Ok(false);
        }
    };
    let english = match message.text_en.as_deref() {
        Some(raw) if broadcast::english_on(&mut tx).await? => {
            match clean_text(&rendered(raw, Locale::En), false) {
                Ok(text) => Some(text),
                Err(err) => {
                    store::log(
                        &mut tx,
                        row.id,
                        "message_english_skipped",
                        &format!(
                            "{}: текст на английском не отправлен ({err}), клиенты с английским получат русский",
                            audience.title(message.only_eligible)
                        ),
                        serde_json::json!({ "message_id": message_id }),
                    )
                    .await?;
                    None
                }
            }
        }
        _ => None,
    };
    let mut chats = store::audience_chats(&mut tx, row.id, audience).await?;
    if message.only_eligible && audience == Audience::Outsiders {
        let conditions = parse_conditions(&row)?;
        let ids: Vec<i64> = chats.iter().map(|c| c.client.0).collect();
        let facts = facts_for(&mut tx, &conditions, row.starts_at, row.ends_at, now, &ids).await?;
        let suspects = antibot::blocked_among(&mut tx, Target::Contest, &ids).await?;
        chats.retain(|chat| {
            facts.get(&chat.client.0).is_some_and(|f| {
                !f.blocked
                    && !suspects.contains_key(&chat.client)
                    && checks(&conditions, f, row.starts_at, row.ends_at, offset)
                        .iter()
                        .all(|c| c.ok)
            })
        });
    }
    let buttons = serde_json::to_value(vec![audience.button()])
        .map_err(|e| ModuleError::Core(e.to_string()))?;
    let (english_chats, russian_chats): (Vec<store::Chat>, Vec<store::Chat>) = chats
        .into_iter()
        .partition(|chat| english.is_some() && chat.locale == Locale::En);
    let batch = Batch {
        message_id,
        buttons: &buttons,
        now,
    };
    let mut added = queue_batch(&mut tx, &batch, Locale::Ru, &text, &russian_chats).await?;
    let mut added_en = 0u64;
    if let Some(english) = &english {
        added_en = queue_batch(&mut tx, &batch, Locale::En, english, &english_chats).await?;
        added += added_en;
    }
    let recipients = i64::try_from(added).unwrap_or(i64::MAX);
    let recipients_en = i64::try_from(added_en).unwrap_or(i64::MAX);
    store::mark_message(&mut tx, message_id, now, recipients, None).await?;
    let queued = if recipients_en > 0 {
        format!("в очереди {recipients}, из них на английском {recipients_en}")
    } else {
        format!("в очереди {recipients}")
    };
    store::log(
        &mut tx,
        row.id,
        "message_queued",
        &format!("{}: {queued}", audience.title(message.only_eligible)),
        serde_json::json!({
            "message_id": message_id,
            "recipients": recipients,
            "english": recipients_en,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

struct Batch<'a> {
    message_id: i64,
    buttons: &'a serde_json::Value,
    now: DateTime<Utc>,
}

async fn queue_batch(
    db: &mut impl Db,
    batch: &Batch<'_>,
    locale: Locale,
    text: &str,
    chats: &[store::Chat],
) -> Result<u64, ModuleError> {
    if chats.is_empty() {
        return Ok(0);
    }
    let rows: Vec<(ClientId, i64)> = chats.iter().map(|c| (c.client, c.chat)).collect();
    let id = broadcast::contest_batch(db, batch.message_id, locale, text, batch.buttons).await?;
    let added = broadcast::queue_bot(db, id, &rows).await?;
    if added > 0 {
        broadcast::open_batch(db, id, batch.now).await?;
    }
    Ok(added)
}

pub struct ContestsModule;

#[async_trait]
impl Module for ContestsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Contests
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn bot_entries(&self) -> Vec<BotEntry> {
        vec![BotEntry {
            key: BOT_ENTRY,
            label: "Конкурсы",
            order: 50,
        }]
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![ScheduledTask {
            name: TASK,
            every: std::time::Duration::from_secs(TASK_EVERY_SECS),
        }]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        if task != TASK {
            return Err(ModuleError::Invalid(format!("у модуля нет задачи {task}")));
        }
        let report = run(ctx).await?;
        if report.drawn + report.prizes + report.messages > 0 {
            tracing::info!(
                drawn = report.drawn,
                prizes = report.prizes,
                messages = report.messages,
                "конкурсы: обход"
            );
        }
        Ok(())
    }
}
