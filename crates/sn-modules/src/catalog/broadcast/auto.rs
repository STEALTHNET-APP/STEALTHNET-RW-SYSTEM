use super::auto_store::{self, Bounds, NewCase, Probe, RuleRow, Tally};
use super::store::{self, ImageMeta, Problem, Progress};
use super::{
    BroadcastError, ButtonSpec, Channel, DAYS_MAX, Lane, clean_buttons, clean_subject, clean_text,
    invalid, mail_site, rejected,
};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::{Languages, Locale};
use sn_domain::format::plural_ru;
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::settings::keys;
use sn_store::{ActorKind, AuditEntry, ClientConditions, Db, StoreError};
use std::collections::{HashMap, HashSet};

pub const AUTO_TASK: &str = "auto";
pub const AUTO_EVERY_SECS: u64 = 300;
pub const CATCH_UP_HOURS: i64 = 24;
pub const RULE_ENTITY: &str = "broadcast_rule";
pub const RULES_MAX: usize = 50;
pub const HOURS_MAX: u16 = 168;
pub const PERCENT_MIN: u8 = 10;
pub const PERCENT_MAX: u8 = 100;
pub const PERCENT_DEFAULT: u8 = 80;
const BATCH: i64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    NoSubscription,
    TrialExpiring,
    Expiring,
    TrafficLow,
    TrialEnded,
    Expired,
    NoPurchase,
    NotConnected,
    UnpaidOrder,
    FirstPurchase,
    Idle,
    DevicesFull,
}

impl Trigger {
    pub const ALL: [Trigger; 12] = [
        Trigger::NoSubscription,
        Trigger::TrialExpiring,
        Trigger::Expiring,
        Trigger::TrafficLow,
        Trigger::TrialEnded,
        Trigger::Expired,
        Trigger::NoPurchase,
        Trigger::NotConnected,
        Trigger::UnpaidOrder,
        Trigger::FirstPurchase,
        Trigger::Idle,
        Trigger::DevicesFull,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Trigger::NoSubscription => "no_subscription",
            Trigger::TrialExpiring => "trial_expiring",
            Trigger::Expiring => "expiring",
            Trigger::TrafficLow => "traffic_low",
            Trigger::TrialEnded => "trial_ended",
            Trigger::Expired => "expired",
            Trigger::NoPurchase => "no_purchase",
            Trigger::NotConnected => "not_connected",
            Trigger::UnpaidOrder => "unpaid_order",
            Trigger::FirstPurchase => "first_purchase",
            Trigger::Idle => "idle",
            Trigger::DevicesFull => "devices_full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Trigger::ALL.into_iter().find(|t| t.as_str() == value)
    }

    pub const fn title(self) -> &'static str {
        match self {
            Trigger::NoSubscription => "Не взял ни пробный период, ни подписку",
            Trigger::TrialExpiring => "Пробный период скоро закончится",
            Trigger::Expiring => "Подписка скоро закончится",
            Trigger::TrafficLow => "Заканчивается трафик",
            Trigger::TrialEnded => "Пробный период закончился, покупки нет",
            Trigger::Expired => "Подписка закончилась и не продлена",
            Trigger::NoPurchase => "Нет покупок после регистрации",
            Trigger::NotConnected => "Нет подключений после покупки",
            Trigger::UnpaidOrder => "Счёт создан и не оплачен",
            Trigger::FirstPurchase => "Первая покупка",
            Trigger::Idle => "Подписка действует, но давно нет подключений",
            Trigger::DevicesFull => "Заняты все устройства",
        }
    }

    pub const fn min_amount(self) -> u16 {
        match self {
            Trigger::Expired | Trigger::TrialEnded | Trigger::TrafficLow | Trigger::DevicesFull => {
                0
            }
            _ => 1,
        }
    }

    pub const fn fixed(self) -> bool {
        matches!(self, Trigger::DevicesFull)
    }

    pub const fn has_tariff(self) -> bool {
        !matches!(self, Trigger::NoPurchase | Trigger::NoSubscription)
    }

    pub const fn has_autorenew(self) -> bool {
        !matches!(
            self,
            Trigger::NoPurchase | Trigger::NoSubscription | Trigger::UnpaidOrder
        )
    }

    pub const fn by_percent(self) -> bool {
        matches!(self, Trigger::TrafficLow)
    }

    pub fn timing(self, delay: Delay, percent: Option<u8>) -> String {
        let span = delay.words();
        match self {
            Trigger::TrafficLow => format!(
                "когда израсходовано {}% трафика",
                percent.unwrap_or(PERCENT_DEFAULT)
            ),
            Trigger::Expiring | Trigger::TrialExpiring => format!("за {span} до окончания"),
            Trigger::Expired | Trigger::TrialEnded if delay.amount == 0 => {
                "сразу после окончания".into()
            }
            Trigger::Expired | Trigger::TrialEnded => format!("через {span} после окончания"),
            Trigger::NoPurchase | Trigger::NoSubscription => {
                format!("через {span} после регистрации")
            }
            Trigger::NotConnected => format!("через {span} после покупки"),
            Trigger::UnpaidOrder => format!("через {span} после создания счёта"),
            Trigger::FirstPurchase => format!("через {span} после первой покупки"),
            Trigger::Idle => format!("после {span} без подключений"),
            Trigger::DevicesFull => "когда заняты все устройства".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Hours,
    #[default]
    Days,
}

impl Unit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Unit::Hours => "hours",
            Unit::Days => "days",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Unit::Hours, Unit::Days]
            .into_iter()
            .find(|u| u.as_str() == value)
    }

    pub const fn max(self) -> u16 {
        match self {
            Unit::Hours => HOURS_MAX,
            Unit::Days => DAYS_MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delay {
    pub amount: u16,
    pub unit: Unit,
}

impl Delay {
    pub const fn days(amount: u16) -> Self {
        Delay {
            amount,
            unit: Unit::Days,
        }
    }

    pub const fn hours(amount: u16) -> Self {
        Delay {
            amount,
            unit: Unit::Hours,
        }
    }

    pub fn span(self) -> Duration {
        match self.unit {
            Unit::Hours => Duration::hours(i64::from(self.amount)),
            Unit::Days => Duration::days(i64::from(self.amount)),
        }
    }

    pub fn words(self) -> String {
        let n = u64::from(self.amount);
        match self.unit {
            Unit::Hours => format!("{n} {}", plural_ru(n, "час", "часа", "часов")),
            Unit::Days => format!("{n} {}", plural_ru(n, "день", "дня", "дней")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Localized {
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub buttons: Vec<ButtonSpec>,
}

impl Localized {
    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
            && self.buttons.is_empty()
            && self.subject.as_deref().is_none_or(|s| s.trim().is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TriggerOption {
    pub key: Trigger,
    pub title: &'static str,
    pub min: u16,
    pub percent: bool,
    pub fixed: bool,
    pub tariff: bool,
    pub autorenew: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutoOptions {
    pub every_minutes: u64,
    pub catch_up_hours: i64,
    pub rules_max: usize,
    pub days_max: u16,
    pub hours_max: u16,
    pub percent_min: u8,
    pub percent_max: u8,
    pub percent_default: u8,
    pub english: bool,
    pub triggers: Vec<TriggerOption>,
}

pub async fn english_on(db: &mut impl Db) -> Result<bool, StoreError> {
    Ok(db
        .settings()
        .get(keys::LANGUAGES)
        .await?
        .and_then(|value| serde_json::from_value::<Languages>(value).ok())
        .is_some_and(|languages| languages.english))
}

pub async fn options(db: &mut impl Db) -> Result<AutoOptions, StoreError> {
    Ok(AutoOptions {
        every_minutes: AUTO_EVERY_SECS / 60,
        catch_up_hours: CATCH_UP_HOURS,
        rules_max: RULES_MAX,
        days_max: DAYS_MAX,
        hours_max: HOURS_MAX,
        percent_min: PERCENT_MIN,
        percent_max: PERCENT_MAX,
        percent_default: PERCENT_DEFAULT,
        english: english_on(db).await?,
        triggers: Trigger::ALL
            .into_iter()
            .map(|key| TriggerOption {
                key,
                title: key.title(),
                min: key.min_amount(),
                percent: key.by_percent(),
                fixed: key.fixed(),
                tariff: key.has_tariff(),
                autorenew: key.has_autorenew(),
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuleDraft {
    pub trigger: Trigger,
    #[serde(default, alias = "days")]
    pub amount: u16,
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub percent: Option<u8>,
    #[serde(default)]
    pub with_autorenew: bool,
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
    #[serde(default)]
    pub en: Option<Localized>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub audience: Option<ClientConditions>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleContent {
    pub trigger: Trigger,
    pub delay: Delay,
    pub percent: Option<u8>,
    pub with_autorenew: bool,
    pub channel: Channel,
    pub subject: Option<String>,
    pub text: String,
    pub image_id: Option<i64>,
    pub buttons: Vec<ButtonSpec>,
    pub en: Option<Localized>,
    pub audience: Option<ClientConditions>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RuleStats {
    pub cases: i64,
    pub unreachable: i64,
    pub progress: Progress,
    pub last_sent_at: Option<DateTime<Utc>>,
}

impl From<Tally> for RuleStats {
    fn from(tally: Tally) -> Self {
        RuleStats {
            cases: tally.cases,
            unreachable: tally.unreachable,
            progress: tally.progress,
            last_sent_at: tally.last_sent_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rule {
    pub id: i64,
    pub trigger: Trigger,
    pub amount: u16,
    pub unit: Unit,
    pub percent: Option<u8>,
    pub with_autorenew: bool,
    pub title: &'static str,
    pub timing: String,
    pub channel: Channel,
    pub subject: Option<String>,
    pub text: String,
    pub image: Option<ImageMeta>,
    pub buttons: Vec<ButtonSpec>,
    pub en: Option<Localized>,
    pub audience: Option<ClientConditions>,
    pub audience_label: Option<String>,
    pub preset: Option<String>,
    pub enabled: bool,
    pub notice: Option<String>,
    pub sending_notices: Vec<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stats: RuleStats,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleItem {
    pub id: i64,
    pub trigger: Trigger,
    pub amount: u16,
    pub unit: Unit,
    pub percent: Option<u8>,
    pub with_autorenew: bool,
    pub title: &'static str,
    pub timing: String,
    pub channel: Channel,
    pub preview: String,
    pub has_image: bool,
    pub buttons: usize,
    pub english: bool,
    pub audience_label: Option<String>,
    pub preset: Option<String>,
    pub enabled: bool,
    pub notice: Option<String>,
    pub stats: RuleStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReachQuery {
    pub trigger: Trigger,
    #[serde(default, alias = "days")]
    pub amount: u16,
    #[serde(default)]
    pub unit: Unit,
    #[serde(default)]
    pub percent: Option<u8>,
    #[serde(default)]
    pub with_autorenew: bool,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub audience: Option<ClientConditions>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AutoReach {
    pub cases: i64,
    pub recipients: i64,
    pub stopped: i64,
    pub emails: i64,
    pub unsubscribed: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CheckReport {
    pub rules: usize,
    pub cases: usize,
    pub queued: usize,
}

fn check_when(
    trigger: Trigger,
    amount: u16,
    unit: Unit,
    percent: Option<u8>,
) -> Result<(Delay, Option<u8>), BroadcastError> {
    if trigger.by_percent() {
        let value = percent.unwrap_or(PERCENT_DEFAULT);
        if !(PERCENT_MIN..=PERCENT_MAX).contains(&value) {
            return Err(invalid(format!(
                "Порог: от {PERCENT_MIN} до {PERCENT_MAX}% трафика"
            )));
        }
        return Ok((Delay::days(0), Some(value)));
    }
    if trigger.fixed() {
        return Ok((Delay::days(0), None));
    }
    let min = trigger.min_amount();
    let max = unit.max();
    if amount < min || amount > max {
        let what = match unit {
            Unit::Hours => "часов",
            Unit::Days => "дней",
        };
        return Err(invalid(format!("Когда: число {what} от {min} до {max}")));
    }
    let delay = Delay { amount, unit };
    if trigger == Trigger::Idle && delay.span() < Duration::days(1) {
        return Err(invalid("Когда: без подключений — не меньше суток"));
    }
    Ok((delay, None))
}

fn english(err: BroadcastError) -> BroadcastError {
    match err {
        BroadcastError::Rejected { code, message } => BroadcastError::Rejected {
            code,
            message: format!("Текст на английском. {message}"),
        },
        other => other,
    }
}

fn clean_en(
    en: Option<&Localized>,
    channel: Channel,
    with_image: bool,
) -> Result<Option<Localized>, BroadcastError> {
    let Some(en) = en.filter(|en| !en.is_blank()) else {
        return Ok(None);
    };
    let subject = match en.subject.as_deref().map(str::trim) {
        Some(subject) if !subject.is_empty() => {
            clean_subject(Some(subject), channel).map_err(english)?
        }
        _ => None,
    };
    Ok(Some(Localized {
        subject,
        text: clean_text(&en.text, with_image).map_err(english)?,
        buttons: clean_buttons(&en.buttons).map_err(english)?,
    }))
}

pub fn check_audience(
    trigger: Trigger,
    audience: Option<&ClientConditions>,
) -> Result<Option<ClientConditions>, BroadcastError> {
    let Some(audience) = audience.filter(|a| !a.is_empty()) else {
        return Ok(None);
    };
    audience
        .check()
        .map_err(|message| invalid(format!("Кому: {message}")))?;
    if audience.status.is_some()
        || audience.ends.is_some()
        || audience.ended.is_some()
        || audience.from.is_some()
        || audience.to.is_some()
    {
        return Err(invalid(
            "Кому: состояние подписки и сроки задаёт само правило, в «Кому» их нет",
        ));
    }
    if !trigger.has_tariff() && (!audience.tariff.is_empty() || !audience.category.is_empty()) {
        return Err(invalid(
            "Кому: у этого события нет подписки, тариф и категория не подходят",
        ));
    }
    if !trigger.has_autorenew() && audience.autorenew.is_some() {
        return Err(invalid(
            "Кому: у этого события нет подписки с автопродлением, это условие не подходит",
        ));
    }
    Ok(Some(audience.clone()))
}

async fn cleaned(db: &mut impl Db, draft: &RuleDraft) -> Result<RuleContent, BroadcastError> {
    let (delay, percent) = check_when(draft.trigger, draft.amount, draft.unit, draft.percent)?;
    if let Some(id) = draft.image_id
        && !store::image_exists(db, id).await?
    {
        return Err(rejected(
            "broadcast.image",
            "Картинка не найдена: загрузите её заново",
        ));
    }
    let with_image = draft.image_id.is_some();
    Ok(RuleContent {
        trigger: draft.trigger,
        delay,
        percent,
        with_autorenew: draft.with_autorenew && draft.trigger == Trigger::Expiring,
        channel: draft.channel,
        subject: clean_subject(draft.subject.as_deref(), draft.channel)?,
        text: clean_text(&draft.text, with_image)?,
        image_id: draft.image_id,
        buttons: clean_buttons(&draft.buttons)?,
        en: clean_en(draft.en.as_ref(), draft.channel, with_image)?,
        audience: check_audience(draft.trigger, draft.audience.as_ref())?,
    })
}

pub(super) fn ready(
    ctx: &ModuleContext,
    channel: Channel,
    enabled: bool,
) -> Result<(), BroadcastError> {
    if enabled && channel.has(Lane::Email) {
        mail_site(ctx)?;
    }
    Ok(())
}

pub(super) fn audit(
    action: &str,
    admin: Option<AdminId>,
    id: i64,
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
        entity_kind: Some(RULE_ENTITY.into()),
        entity_id: Some(id),
        details,
        ip: None,
    }
}

async fn names(
    db: &mut impl Db,
    rows: &[&RuleRow],
) -> Result<Option<super::AudienceNames>, BroadcastError> {
    if rows.iter().all(|row| row.audience.is_none()) {
        return Ok(None);
    }
    let options = sn_store::filter_options(db.conn()).await?;
    Ok(Some(super::AudienceNames {
        tariffs: options.tariff_names(),
        categories: options.category_names(),
        currency: crate::catalog::installation_currency(db).await?,
    }))
}

fn audience_label(
    audience: Option<&ClientConditions>,
    names: Option<&super::AudienceNames>,
) -> Option<String> {
    let audience = audience?;
    let label = super::Audience::Filter(audience.clone()).describe(None, names);
    Some(label)
}

async fn view(db: &mut impl Db, id: i64) -> Result<Rule, BroadcastError> {
    let row = auto_store::row(db, id).await?;
    let parsed = auto_store::parse(&row)?;
    let names = names(db, &[&row]).await?;
    let image = match row.image_id {
        Some(image) => store::image_meta(db, image).await?,
        None => None,
    };
    let stats = auto_store::tallies(db, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default()
        .into();
    let problems = auto_store::problems(db, id).await?;
    let sending_notices = auto_store::sending_notices(db, id).await?;
    Ok(Rule {
        id: row.id,
        trigger: parsed.trigger,
        amount: parsed.delay.amount,
        unit: parsed.delay.unit,
        percent: parsed.percent,
        with_autorenew: row.with_autorenew,
        title: parsed.trigger.title(),
        timing: parsed.trigger.timing(parsed.delay, parsed.percent),
        channel: parsed.channel,
        subject: row.subject,
        text: row.text,
        image,
        buttons: parsed.buttons,
        en: parsed.en,
        audience_label: audience_label(parsed.audience.as_ref(), names.as_ref()),
        audience: parsed.audience,
        preset: row.preset,
        enabled: row.enabled,
        notice: row.notice,
        sending_notices,
        checked_at: row.checked_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        stats,
        problems,
    })
}

fn item(
    row: &RuleRow,
    tally: Tally,
    names: Option<&super::AudienceNames>,
) -> Result<RuleItem, BroadcastError> {
    let parsed = auto_store::parse(row)?;
    Ok(RuleItem {
        id: row.id,
        trigger: parsed.trigger,
        amount: parsed.delay.amount,
        unit: parsed.delay.unit,
        percent: parsed.percent,
        with_autorenew: row.with_autorenew,
        title: parsed.trigger.title(),
        timing: parsed.trigger.timing(parsed.delay, parsed.percent),
        channel: parsed.channel,
        preview: store::preview(&row.text),
        has_image: row.image_id.is_some(),
        buttons: parsed.buttons.len(),
        english: parsed.en.is_some(),
        audience_label: audience_label(parsed.audience.as_ref(), names),
        preset: row.preset.clone(),
        enabled: row.enabled,
        notice: row.notice.clone(),
        stats: tally.into(),
    })
}

pub async fn rules(ctx: &ModuleContext) -> Result<Vec<RuleItem>, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    let rows = auto_store::rows(&mut conn).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut tallies = auto_store::tallies(&mut conn, &ids).await?;
    let names = names(&mut conn, &rows.iter().collect::<Vec<_>>()).await?;
    rows.iter()
        .map(|row| {
            item(
                row,
                tallies.remove(&row.id).unwrap_or_default(),
                names.as_ref(),
            )
        })
        .collect()
}

pub async fn rule(ctx: &ModuleContext, id: i64) -> Result<Rule, BroadcastError> {
    let mut conn = ctx.store.conn().await?;
    view(&mut conn, id).await
}

fn details(content: &RuleContent, enabled: bool) -> serde_json::Value {
    serde_json::json!({
        "trigger": content.trigger,
        "amount": content.delay.amount,
        "unit": content.delay.unit,
        "percent": content.percent,
        "channel": content.channel,
        "english": content.en.is_some(),
        "audience": content.audience,
        "enabled": enabled,
    })
}

pub async fn create_rule(
    ctx: &ModuleContext,
    draft: &RuleDraft,
    admin: Option<AdminId>,
) -> Result<Rule, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let content = cleaned(&mut tx, draft).await?;
    if auto_store::rows(&mut tx).await?.len() >= RULES_MAX {
        return Err(invalid(format!(
            "Правил не больше {RULES_MAX}: удалите ненужные"
        )));
    }
    ready(ctx, content.channel, draft.enabled)?;
    let id = auto_store::insert(&mut tx, &content, draft.enabled, None, admin).await?;
    tx.audit()
        .record(&audit(
            "broadcast.auto_created",
            admin,
            id,
            details(&content, draft.enabled),
        ))
        .await?;
    let view = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn update_rule(
    ctx: &ModuleContext,
    id: i64,
    draft: &RuleDraft,
    admin: Option<AdminId>,
) -> Result<Rule, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let before = auto_store::lock(&mut tx, id).await?;
    let content = cleaned(&mut tx, draft).await?;
    ready(ctx, content.channel, draft.enabled)?;
    auto_store::update(&mut tx, id, &content, draft.enabled).await?;
    if before.enabled && !draft.enabled {
        auto_store::stop_queue(&mut tx, id).await?;
    }
    tx.audit()
        .record(&audit(
            "broadcast.auto_updated",
            admin,
            id,
            details(&content, draft.enabled),
        ))
        .await?;
    let view = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn switch_rule(
    ctx: &ModuleContext,
    id: i64,
    enabled: bool,
    admin: Option<AdminId>,
) -> Result<Rule, BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    let row = auto_store::lock(&mut tx, id).await?;
    if row.enabled != enabled {
        let parsed = auto_store::parse(&row)?;
        ready(ctx, parsed.channel, enabled)?;
        auto_store::set_enabled(&mut tx, id, enabled).await?;
        if !enabled {
            auto_store::stop_queue(&mut tx, id).await?;
        }
        tx.audit()
            .record(&audit(
                if enabled {
                    "broadcast.auto_enabled"
                } else {
                    "broadcast.auto_disabled"
                },
                admin,
                id,
                serde_json::json!({}),
            ))
            .await?;
    }
    let view = view(&mut tx, id).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn delete_rule(
    ctx: &ModuleContext,
    id: i64,
    admin: Option<AdminId>,
) -> Result<(), BroadcastError> {
    let mut tx = ctx.store.begin().await?;
    auto_store::lock(&mut tx, id).await?;
    auto_store::remove(&mut tx, id).await?;
    let stopped = auto_store::stop_queue(&mut tx, id).await?;
    tx.audit()
        .record(&audit(
            "broadcast.auto_deleted",
            admin,
            id,
            serde_json::json!({ "stopped": stopped }),
        ))
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn rule_reach(
    ctx: &ModuleContext,
    query: &ReachQuery,
) -> Result<AutoReach, BroadcastError> {
    let (delay, percent) = check_when(query.trigger, query.amount, query.unit, query.percent)?;
    let audience = check_audience(query.trigger, query.audience.as_ref())?;
    let mut conn = ctx.store.conn().await?;
    let now = crate::catalog::db_now(&mut conn).await?;
    let currency = crate::catalog::installation_currency(&mut conn).await?;
    let probe = Probe {
        trigger: query.trigger,
        delay,
        percent,
        with_autorenew: query.with_autorenew && query.trigger == Trigger::Expiring,
        rule: query.id,
        english: false,
        audience,
        currency,
    };
    let bounds = Bounds::of(
        query.trigger,
        delay.span(),
        now,
        Duration::hours(CATCH_UP_HOURS),
    );
    let counted = auto_store::count(&mut conn, &probe, &bounds).await?;
    Ok(AutoReach {
        cases: counted.cases,
        recipients: counted.recipients,
        stopped: counted.stopped,
        emails: counted.emails,
        unsubscribed: counted.unsubscribed,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct Round {
    cases: usize,
    queued: usize,
    more: bool,
}

pub async fn check(ctx: &ModuleContext, now: DateTime<Utc>) -> Result<CheckReport, ModuleError> {
    let (ids, english) = {
        let mut conn = ctx.store.conn().await?;
        (
            auto_store::enabled_ids(&mut conn).await?,
            english_on(&mut conn).await?,
        )
    };
    let mail = mail_site(ctx).map(|_| ()).map_err(|err| err.to_string());
    let mut report = CheckReport::default();
    for id in ids {
        report.rules += 1;
        loop {
            let round = check_rule(ctx, id, now, &mail, english).await?;
            report.cases += round.cases;
            report.queued += round.queued;
            if !round.more {
                break;
            }
        }
    }
    Ok(report)
}

fn mail_notice(channel: Channel, mail: &Result<(), String>) -> Option<String> {
    match (mail, channel) {
        (Err(reason), Channel::Email) => Some(format!(
            "{reason}. Пока почта не заработает, правило не срабатывает"
        )),
        (Err(reason), Channel::Both) => Some(format!(
            "{reason}. Письма по правилу не уходят, в бот сообщения уходят"
        )),
        _ => None,
    }
}

pub(super) fn traffic_mark(key: &str) -> Option<(i64, i32)> {
    let mut parts = key.split(':');
    if parts.next() != Some("traffic") {
        return None;
    }
    let subscription = parts.next()?.parse().ok()?;
    let seq = parts.next()?.parse().ok()?;
    Some((subscription, seq))
}

async fn check_rule(
    ctx: &ModuleContext,
    id: i64,
    now: DateTime<Utc>,
    mail: &Result<(), String>,
    english: bool,
) -> Result<Round, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let row = match auto_store::lock(&mut tx, id).await {
        Ok(row) if row.enabled => row,
        Ok(_) | Err(StoreError::NotFound(_)) => {
            tx.rollback().await?;
            return Ok(Round::default());
        }
        Err(err) => return Err(err.into()),
    };
    let parsed = auto_store::parse(&row)?;
    let notice = mail_notice(parsed.channel, mail);
    let bot_lane = parsed.channel.has(Lane::Bot);
    let mail_lane = parsed.channel.has(Lane::Email) && mail.is_ok();
    if !bot_lane && !mail_lane {
        auto_store::set_checked(&mut tx, id, now, notice.as_deref()).await?;
        tx.commit().await?;
        return Ok(Round::default());
    }
    if parsed.trigger == Trigger::TrafficLow {
        auto_store::rearm_traffic(&mut tx, id, parsed.percent.unwrap_or(PERCENT_DEFAULT)).await?;
    }
    let currency = match &parsed.audience {
        Some(audience) if audience.uses_money() => {
            crate::catalog::installation_currency(&mut tx).await?
        }
        _ => sn_domain::money::Currency::default(),
    };
    let probe = Probe {
        trigger: parsed.trigger,
        delay: parsed.delay,
        percent: parsed.percent,
        with_autorenew: row.with_autorenew,
        rule: Some(id),
        english: english && parsed.en.is_some(),
        audience: parsed.audience.clone(),
        currency,
    };
    let bounds = Bounds::of(
        parsed.trigger,
        parsed.delay.span(),
        now,
        Duration::hours(CATCH_UP_HOURS),
    );
    let found = auto_store::found(&mut tx, &probe, &bounds, BATCH).await?;
    let full = i64::try_from(found.len()).unwrap_or(i64::MAX) >= BATCH;
    let targets: Vec<(Option<i64>, Option<String>, Locale)> = found
        .iter()
        .map(|f| {
            (
                f.chat().filter(|_| bot_lane),
                f.mailbox().filter(|_| mail_lane).map(str::to_string),
                f.locale(),
            )
        })
        .collect();
    let mut batches: HashMap<Locale, i64> = HashMap::new();
    for (chat, email, locale) in &targets {
        if (chat.is_some() || email.is_some()) && !batches.contains_key(locale) {
            let batch =
                auto_store::batch(&mut tx, id, now.date_naive(), row.version, *locale).await?;
            batches.insert(*locale, batch);
        }
    }
    let cases: Vec<NewCase<'_>> = found
        .iter()
        .zip(&targets)
        .map(|(f, (chat, email, locale))| NewCase {
            key: &f.case_key,
            client_id: ClientId(f.client_id),
            due_at: f.due_at,
            batch: (chat.is_some() || email.is_some())
                .then(|| batches.get(locale).copied())
                .flatten(),
        })
        .collect();
    let recorded: HashSet<String> = auto_store::record_cases(&mut tx, id, &cases)
        .await?
        .into_iter()
        .collect();
    let mut chats: HashMap<i64, Vec<(ClientId, i64)>> = HashMap::new();
    let mut mails: HashMap<i64, Vec<(ClientId, String)>> = HashMap::new();
    let mut seen_chat: HashSet<i64> = HashSet::new();
    let mut seen_mail: HashSet<i64> = HashSet::new();
    let mut fired: Vec<(i64, i32)> = Vec::new();
    let mut queued = 0usize;
    for (f, (chat, email, locale)) in found.iter().zip(&targets) {
        if !recorded.contains(&f.case_key) {
            continue;
        }
        if let Some(mark) = traffic_mark(&f.case_key) {
            fired.push(mark);
        }
        let Some(batch) = batches.get(locale).copied() else {
            continue;
        };
        if chat.is_some() || email.is_some() {
            queued += 1;
        }
        if let Some(chat) = chat
            && seen_chat.insert(f.client_id)
        {
            chats
                .entry(batch)
                .or_default()
                .push((ClientId(f.client_id), *chat));
        }
        if let Some(email) = email
            && seen_mail.insert(f.client_id)
        {
            mails
                .entry(batch)
                .or_default()
                .push((ClientId(f.client_id), email.clone()));
        }
    }
    if !fired.is_empty() {
        auto_store::mark_traffic(&mut tx, id, &fired).await?;
    }
    for batch in batches.values().copied() {
        let added =
            auto_store::queue_bot(&mut tx, batch, chats.get(&batch).map_or(&[], Vec::as_slice))
                .await?
                + auto_store::queue_mail(
                    &mut tx,
                    batch,
                    mails.get(&batch).map_or(&[], Vec::as_slice),
                )
                .await?;
        if added > 0 {
            auto_store::open_batch(&mut tx, batch, now).await?;
        }
    }
    auto_store::set_checked(&mut tx, id, now, notice.as_deref()).await?;
    tx.commit().await?;
    if !recorded.is_empty() {
        tracing::info!(
            rule = id,
            cases = recorded.len(),
            queued,
            "авторассылка: новые случаи"
        );
    }
    Ok(Round {
        cases: recorded.len(),
        queued,
        more: full && !recorded.is_empty(),
    })
}
