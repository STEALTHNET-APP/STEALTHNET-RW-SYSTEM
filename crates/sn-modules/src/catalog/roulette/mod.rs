mod routes;
mod store;
#[cfg(test)]
mod tests;

use crate::catalog::antibot::{self, Guard, Target};
use crate::catalog::prize::{self, Prize, PrizeDraft};
use crate::catalog::promo::PromoKind;
use crate::catalog::{db_now, installation_currency};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::Module;
use async_trait::async_trait;
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId, SubscriptionId};
use sn_domain::ledger::LedgerKind;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_domain::order::{Order, OrderKind};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::Payment;
use sn_store::{ActorKind, AuditEntry, ClientPerson, Db, LockSpace, StoreError, Tx};
use tracing::Instrument;
use uuid::Uuid;

pub use store::Tally;

pub const ENTITY: &str = "roulette";
pub const SECTORS_MIN: usize = 2;
pub const SECTORS_MAX: usize = 12;
pub const WEIGHT_MAX: u32 = 10_000;
pub const FREE_EVERY_MAX: u32 = 365;
pub const HISTORY: i64 = 10;
pub const LABEL_MAX: usize = 24;
pub const FREE_DENIED: &str = "Бесплатные вращения для этого аккаунта недоступны";
pub const CONFIRM_TEXT: &str = "Перед бесплатным вращением подтвердите, что вы не робот.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreeFor {
    All,
    Subscribers,
    #[default]
    Paid,
}

impl FreeFor {
    pub const fn title(self) -> &'static str {
        match self {
            FreeFor::All => "всем клиентам",
            FreeFor::Subscribers => "клиентам с действующей подпиской",
            FreeFor::Paid => "клиентам с купленной или подаренной подпиской",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Day,
    Week,
    #[default]
    Month,
}

impl Period {
    pub const fn span(self) -> Duration {
        match self {
            Period::Day => Duration::days(1),
            Period::Week => Duration::days(7),
            Period::Month => Duration::days(30),
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Period::Day => "за сутки",
            Period::Week => "за 7 дней",
            Period::Month => "за 30 дней",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub period: Period,
    #[serde(default)]
    pub balance: Option<Money>,
    #[serde(default)]
    pub days: Option<u32>,
    #[serde(default)]
    pub promos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sector {
    pub prize: Prize,
    pub weight: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Sector {
    pub fn title(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.prize.label())
    }

    fn stored_title(&self) -> String {
        sn_domain::lang::sync_scope(sn_domain::client::Locale::Ru, || self.title())
    }
}

fn shown_title(stored: String, prize: &Prize) -> String {
    if sn_domain::lang::current() == sn_domain::client::Locale::Ru {
        return stored;
    }
    let standard = sn_domain::lang::sync_scope(sn_domain::client::Locale::Ru, || prize.label());
    if stored == standard {
        prize.label()
    } else {
        stored
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub sectors: Vec<Sector>,
    #[serde(default)]
    pub free_every_days: Option<u32>,
    #[serde(default)]
    pub free_for: FreeFor,
    #[serde(default)]
    pub per_purchase: bool,
    #[serde(default)]
    pub purchase_min: Option<Money>,
    #[serde(default)]
    pub budget: Budget,
}

impl Config {
    pub fn starter(currency: Currency) -> Self {
        let unit = currency.minor_per_unit();
        let small = if currency == Currency::Rub { 50 } else { 1 };
        Config {
            sectors: vec![
                Sector {
                    prize: Prize::Days { days: 3 },
                    weight: 15,
                    label: None,
                },
                Sector {
                    prize: Prize::Empty,
                    weight: 30,
                    label: None,
                },
                Sector {
                    prize: Prize::Balance {
                        amount: Money::new(small * unit, currency),
                    },
                    weight: 10,
                    label: None,
                },
                Sector {
                    prize: Prize::Promo {
                        percent: 10,
                        valid_days: 14,
                    },
                    weight: 20,
                    label: None,
                },
                Sector {
                    prize: Prize::Days { days: 7 },
                    weight: 5,
                    label: None,
                },
                Sector {
                    prize: Prize::Empty,
                    weight: 30,
                    label: None,
                },
            ],
            free_every_days: Some(7),
            free_for: FreeFor::Paid,
            per_purchase: true,
            purchase_min: None,
            budget: Budget::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SectorDraft {
    pub prize: PrizeDraft,
    pub weight: u32,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BudgetDraft {
    #[serde(default)]
    pub period: Period,
    #[serde(default)]
    pub balance: Option<String>,
    #[serde(default)]
    pub days: Option<u32>,
    #[serde(default)]
    pub promos: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConfigDraft {
    pub sectors: Vec<SectorDraft>,
    #[serde(default)]
    pub free_every_days: Option<u32>,
    #[serde(default)]
    pub free_for: FreeFor,
    #[serde(default)]
    pub per_purchase: bool,
    #[serde(default)]
    pub purchase_min: Option<String>,
    pub budget: BudgetDraft,
    #[serde(default)]
    pub version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SectorView {
    pub index: usize,
    pub prize: Prize,
    pub kind: &'static str,
    pub title: String,
    pub custom: bool,
    pub weight: u32,
    pub chance: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Spent {
    pub balance: Money,
    pub days: i64,
    pub promos: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdminView {
    pub config: Config,
    pub sectors: Vec<SectorView>,
    pub version: i32,
    pub saved: bool,
    pub updated_at: Option<DateTime<Utc>>,
    pub currency: Currency,
    pub promo_enabled: bool,
    pub spent: Spent,
    pub tally: Tally,
    pub tickets_open: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryItem {
    pub id: i64,
    pub sector: i32,
    pub title: String,
    pub prize: Prize,
    pub state: String,
    pub note: Option<String>,
    pub code: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Spins {
    pub free_ready: bool,
    pub free_next_at: Option<DateTime<Utc>>,
    pub free_blocked: Option<String>,
    pub purchase: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClientState {
    pub ready: bool,
    pub version: i32,
    pub sectors: Vec<SectorView>,
    pub spins: Spins,
    pub free_every_days: Option<u32>,
    pub free_for: FreeFor,
    pub per_purchase: bool,
    pub purchase_min: Option<Money>,
    pub history: Vec<HistoryItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpinResult {
    pub spin: HistoryItem,
    pub state: ClientState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpinItem {
    pub id: i64,
    pub client_id: ClientId,
    pub client: Option<ClientPerson>,
    pub source: String,
    pub title: String,
    pub prize: Prize,
    pub state: String,
    pub note: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouletteError {
    #[error("{message}")]
    Rejected { code: &'static str, message: String },
    #[error(transparent)]
    Module(#[from] ModuleError),
}

impl From<StoreError> for RouletteError {
    fn from(err: StoreError) -> Self {
        RouletteError::Module(ModuleError::Store(err))
    }
}

fn rejected(code: &'static str, message: impl Into<String>) -> RouletteError {
    RouletteError::Rejected {
        code,
        message: message.into(),
    }
}

fn invalid(message: impl Into<String>) -> ModuleError {
    ModuleError::Invalid(message.into())
}

fn sector_views(config: &Config) -> Vec<SectorView> {
    let total: u64 = config.sectors.iter().map(|s| u64::from(s.weight)).sum();
    config
        .sectors
        .iter()
        .enumerate()
        .map(|(index, s)| SectorView {
            index,
            prize: s.prize.clone(),
            kind: s.prize.kind(),
            title: s.title(),
            custom: s.label.is_some(),
            weight: s.weight,
            chance: if total == 0 {
                0.0
            } else {
                (f64::from(s.weight) * 1000.0 / total as f64).round() / 10.0
            },
        })
        .collect()
}

pub fn validate(draft: &ConfigDraft, currency: Currency) -> Result<Config, ModuleError> {
    if draft.sectors.len() < SECTORS_MIN || draft.sectors.len() > SECTORS_MAX {
        return Err(invalid(format!(
            "Призов в рулетке — от {SECTORS_MIN} до {SECTORS_MAX}"
        )));
    }
    let mut sectors = Vec::with_capacity(draft.sectors.len());
    for (index, sector) in draft.sectors.iter().enumerate() {
        let n = index + 1;
        let prize = prize::validate(&sector.prize, currency, true)
            .map_err(|err| invalid(format!("Приз {n}: {}", strip(&err))))?;
        if matches!(&prize, Prize::Custom { .. }) {
            return Err(invalid(format!(
                "Приз {n}: свой приз можно разыграть только в конкурсе"
            )));
        }
        if sector.weight > WEIGHT_MAX {
            return Err(invalid(format!("Приз {n}: вес — от 0 до {WEIGHT_MAX}")));
        }
        let label = sector
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string);
        if label
            .as_deref()
            .is_some_and(|l| l.chars().count() > LABEL_MAX)
        {
            return Err(invalid(format!(
                "Приз {n}: подпись длиннее {LABEL_MAX} знаков"
            )));
        }
        sectors.push(Sector {
            prize,
            weight: sector.weight,
            label,
        });
    }
    if sectors.iter().all(|s| s.weight == 0) {
        return Err(invalid(
            "Хотя бы у одного приза вес должен быть больше нуля",
        ));
    }
    let free_every_days = draft.free_every_days.filter(|d| *d > 0);
    if free_every_days.is_some_and(|d| d > FREE_EVERY_MAX) {
        return Err(invalid(format!(
            "Бесплатное вращение — не реже раза в {FREE_EVERY_MAX} дней"
        )));
    }
    if free_every_days.is_none() && !draft.per_purchase {
        return Err(invalid(
            "Включите бесплатные вращения или вращения за покупку — иначе крутить нечем",
        ));
    }
    let purchase_min = match draft
        .purchase_min
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(raw) => Some(
            prize::parse_amount(raw, currency)
                .ok_or_else(|| invalid("Покупка от: сумма указана неверно"))?,
        ),
        None => None,
    };
    let balance = match draft
        .budget
        .balance
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(raw) => Some(
            prize::parse_amount(raw, currency)
                .ok_or_else(|| invalid("Бюджет: сумма указана неверно"))?,
        ),
        None => None,
    };
    Ok(Config {
        sectors,
        free_every_days,
        free_for: draft.free_for,
        per_purchase: draft.per_purchase,
        purchase_min,
        budget: Budget {
            period: draft.budget.period,
            balance,
            days: draft.budget.days,
            promos: draft.budget.promos,
        },
    })
}

fn strip(err: &ModuleError) -> String {
    match err {
        ModuleError::Invalid(message) => message.clone(),
        other => other.to_string(),
    }
}

async fn load(db: &mut impl Db) -> Result<(Config, i32, bool, Option<DateTime<Utc>>), ModuleError> {
    let currency = installation_currency(db).await?;
    match store::config(db).await? {
        Some((value, version, updated_at)) => {
            let config: Config = serde_json::from_value(value)
                .map_err(|_| ModuleError::Store(StoreError::Decode("рулетка: настройки".into())))?;
            Ok((config, version, true, Some(updated_at)))
        }
        None => Ok((Config::starter(currency), 0, false, None)),
    }
}

fn audit(action: &str, admin: Option<AdminId>, details: serde_json::Value) -> AuditEntry {
    AuditEntry {
        actor_kind: if admin.is_some() {
            ActorKind::Admin
        } else {
            ActorKind::System
        },
        actor_id: admin.map(|a| a.0),
        action: action.into(),
        entity_kind: Some(ENTITY.into()),
        entity_id: None,
        details,
        ip: None,
    }
}

async fn admin_view(db: &mut impl Db) -> Result<AdminView, ModuleError> {
    let now = db_now(db).await?;
    let currency = installation_currency(db).await?;
    let (config, version, saved, updated_at) = load(db).await?;
    let since = now - config.budget.period.span();
    let spent = store::spent(db, since).await?;
    Ok(AdminView {
        sectors: sector_views(&config),
        spent: Spent {
            balance: Money::new(spent.balance_minor, currency),
            days: spent.days,
            promos: spent.promo,
        },
        tally: store::tally(db, since).await?,
        tickets_open: store::tickets_open(db).await?,
        promo_enabled: prize::promo_enabled(db).await?,
        config,
        version,
        saved,
        updated_at,
        currency,
    })
}

pub async fn settings(ctx: &ModuleContext) -> Result<AdminView, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    admin_view(&mut conn).await
}

pub async fn save(
    ctx: &ModuleContext,
    draft: &ConfigDraft,
    admin: Option<AdminId>,
) -> Result<AdminView, ModuleError> {
    let mut tx = ctx.store.begin().await?;
    let currency = installation_currency(&mut tx).await?;
    let config = validate(draft, currency)?;
    if config
        .sectors
        .iter()
        .any(|s| matches!(&s.prize, Prize::Promo { .. }) && s.weight > 0)
        && !prize::promo_enabled(&mut tx).await?
    {
        return Err(invalid(
            "Промокод в рулетке работает, только когда включён модуль «Промокоды»",
        ));
    }
    let current = store::lock_config(&mut tx).await?.unwrap_or(0);
    if let Some(expected) = draft.version
        && expected != current
    {
        return Err(invalid(
            "Настройки рулетки только что изменил кто-то другой. Обновите страницу",
        ));
    }
    let value = serde_json::to_value(&config).map_err(|e| ModuleError::Core(e.to_string()))?;
    let version = store::save_config(&mut tx, &value, admin).await?;
    tx.audit()
        .record(&audit(
            "roulette.saved",
            admin,
            serde_json::json!({ "version": version, "sectors": config.sectors.len() }),
        ))
        .await?;
    let view = admin_view(&mut tx).await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn spins(ctx: &ModuleContext, page: PageRequest) -> Result<Page<SpinItem>, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    let rows = store::recent(&mut conn, page).await?;
    let ids: Vec<ClientId> = rows.items.iter().map(|r| ClientId(r.client_id)).collect();
    let people = conn.clients().people(&ids).await?;
    let items = rows
        .items
        .into_iter()
        .map(|r| {
            let prize: Prize = serde_json::from_value(r.prize.clone()).unwrap_or(Prize::Empty);
            SpinItem {
                id: r.id,
                client_id: ClientId(r.client_id),
                client: people.get(&ClientId(r.client_id)).cloned(),
                source: r.source,
                title: r.label,
                prize,
                state: r.state,
                note: r.note,
                code: r.code,
                error: r.error,
                at: r.created_at,
            }
        })
        .collect();
    Ok(Page {
        items,
        next: rows.next,
    })
}

fn free_denied() -> String {
    sn_domain::lang::pick(FREE_DENIED, "Free spins aren't available for this account").to_string()
}

fn free_reason(free_for: FreeFor) -> String {
    match (sn_domain::lang::current(), free_for) {
        (sn_domain::client::Locale::En, FreeFor::All) => "Free spins are for all clients".into(),
        (sn_domain::client::Locale::En, FreeFor::Subscribers) => {
            "Free spins are for clients with an active subscription".into()
        }
        (sn_domain::client::Locale::En, FreeFor::Paid) => {
            "Free spins are for clients with a subscription they bought or received as a gift"
                .into()
        }
        (sn_domain::client::Locale::Ru, free_for) => {
            format!("Бесплатно крутить можно {}", free_for.title())
        }
    }
}

fn shown_note(note: String) -> String {
    if sn_domain::lang::current() == sn_domain::client::Locale::Ru {
        return note;
    }
    match note.as_str() {
        "Дни уже добавлены к подписке" => {
            "The days are already added to your subscription".into()
        }
        "Деньги уже на балансе" => "The money is already on your balance".into(),
        other => match other.strip_prefix("Промокод действует ") {
            Some(rest) => {
                let days: u32 = rest
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                format!(
                    "The promo code works for {}",
                    format::days_in(days, sn_domain::client::Locale::En)
                )
            }
            None => note,
        },
    }
}

fn history_item(row: store::SpinRow) -> HistoryItem {
    let prize: Prize = serde_json::from_value(row.prize).unwrap_or(Prize::Empty);
    let title = shown_title(row.label, &prize);
    HistoryItem {
        id: row.id,
        sector: row.sector,
        prize,
        title,
        state: row.state,
        note: row.note.map(shown_note),
        code: row.code,
        at: row.created_at,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Free {
    ready: bool,
    next_at: Option<DateTime<Utc>>,
    blocked: Option<String>,
    held: bool,
}

async fn free_status(
    db: &mut impl Db,
    config: &Config,
    client_id: ClientId,
    now: DateTime<Utc>,
    denied: bool,
) -> Result<Free, ModuleError> {
    let Some(every) = config.free_every_days else {
        return Ok(Free::default());
    };
    let allowed = match config.free_for {
        FreeFor::All => true,
        FreeFor::Subscribers => store::eligible_free(db, client_id, now, false).await?,
        FreeFor::Paid => store::eligible_free(db, client_id, now, true).await?,
    };
    if !allowed {
        return Ok(Free {
            blocked: Some(if denied {
                free_denied()
            } else {
                free_reason(config.free_for)
            }),
            ..Free::default()
        });
    }
    let next = store::last_free(db, client_id)
        .await?
        .map(|last| last + Duration::days(i64::from(every)));
    let ready = next.is_none_or(|at| at <= now);
    if denied {
        return Ok(Free {
            blocked: Some(free_denied()),
            held: ready,
            ..Free::default()
        });
    }
    Ok(Free {
        ready,
        next_at: next.filter(|at| *at > now),
        blocked: None,
        held: false,
    })
}

async fn client_state(
    db: &mut impl Db,
    client_id: ClientId,
    denied: bool,
) -> Result<(ClientState, bool), ModuleError> {
    let now = db_now(db).await?;
    let (config, version, saved, _) = load(db).await?;
    let free = if saved {
        free_status(db, &config, client_id, now, denied).await?
    } else {
        Free::default()
    };
    let purchase = if config.per_purchase || store::open_tickets(db, client_id).await? > 0 {
        store::open_tickets(db, client_id).await?
    } else {
        0
    };
    let history = store::history(db, client_id, HISTORY)
        .await?
        .into_iter()
        .map(history_item)
        .collect();
    let state = ClientState {
        ready: saved,
        version,
        sectors: sector_views(&config),
        spins: Spins {
            free_ready: free.ready,
            free_next_at: free.next_at,
            free_blocked: free.blocked,
            purchase,
            total: purchase + i64::from(free.ready),
        },
        free_every_days: config.free_every_days,
        free_for: config.free_for,
        per_purchase: config.per_purchase,
        purchase_min: config.purchase_min,
        history,
    };
    Ok((state, free.held))
}

pub async fn ready(ctx: &ModuleContext) -> Result<bool, ModuleError> {
    let mut conn = ctx.store.conn().await?;
    Ok(store::config(&mut conn).await?.is_some())
}

fn target() -> serde_json::Value {
    serde_json::json!({ "target": Target::Roulette.as_str() })
}

pub async fn state(ctx: &ModuleContext, client_id: ClientId) -> Result<ClientState, ModuleError> {
    let guard = antibot::guard(ctx, client_id, Target::Roulette).await?;
    let (state, held) = {
        let mut conn = ctx.store.conn().await?;
        client_state(&mut conn, client_id, Guard::denied(guard.as_ref())).await?
    };
    if held {
        antibot::record(
            ctx,
            client_id,
            guard.as_ref(),
            antibot::Action::RouletteDenied,
            target(),
            None,
        )
        .await?;
    }
    Ok(state)
}

fn available(
    sector: &Sector,
    has_subscription: bool,
    promo_on: bool,
    budget: &Budget,
    spent: &store::Spent,
) -> bool {
    if sector.weight == 0 {
        return false;
    }
    match &sector.prize {
        Prize::Empty => true,
        Prize::Days { days } => {
            has_subscription
                && budget
                    .days
                    .is_none_or(|limit| spent.days + i64::from(*days) <= i64::from(limit))
        }
        Prize::Balance { amount } => budget
            .balance
            .is_none_or(|limit| spent.balance_minor + amount.minor <= limit.minor),
        Prize::Promo { .. } => {
            promo_on
                && budget
                    .promos
                    .is_none_or(|limit| spent.promo < i64::from(limit))
        }
        Prize::Custom { .. } => false,
    }
}

pub fn pick(weights: &[(usize, u32)], roll: u64) -> Option<usize> {
    let total: u64 = weights.iter().map(|(_, w)| u64::from(*w)).sum();
    if total == 0 {
        return None;
    }
    let mut point = roll % total;
    for (index, weight) in weights {
        let weight = u64::from(*weight);
        if point < weight {
            return Some(*index);
        }
        point -= weight;
    }
    None
}

pub async fn spin_confirmed(
    ctx: &ModuleContext,
    client_id: ClientId,
    request_id: Uuid,
    prefer: Option<SubscriptionId>,
    token: &str,
) -> Result<SpinResult, RouletteError> {
    antibot::confirm(ctx, client_id, token, Target::Roulette.as_str()).await?;
    spin(ctx, client_id, request_id, prefer).await
}

pub async fn spin(
    ctx: &ModuleContext,
    client_id: ClientId,
    request_id: Uuid,
    prefer: Option<SubscriptionId>,
) -> Result<SpinResult, RouletteError> {
    let ctx = ctx.clone();
    let work = async move { spin_now(&ctx, client_id, request_id, prefer).await };
    match tokio::spawn(work.in_current_span()).await {
        Ok(result) => result,
        Err(err) => match err.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(err) => Err(RouletteError::Module(ModuleError::Core(format!(
                "вращение прервано: {err}"
            )))),
        },
    }
}

async fn spin_now(
    ctx: &ModuleContext,
    client_id: ClientId,
    request_id: Uuid,
    prefer: Option<SubscriptionId>,
) -> Result<SpinResult, RouletteError> {
    let guard = antibot::guard(ctx, client_id, Target::Roulette).await?;
    let denied = Guard::denied(guard.as_ref());
    let mut tx = ctx.store.begin().await?;
    tx.lock(LockSpace::Client, client_id.0).await?;
    if let Some(found) = store::spin_by_request(&mut tx, client_id, request_id).await? {
        let (state, _) = client_state(&mut tx, client_id, denied).await?;
        tx.commit().await?;
        return Ok(SpinResult {
            spin: history_item(found),
            state,
        });
    }
    let now = db_now(&mut tx).await?;
    match store::client_state(&mut tx, client_id).await? {
        Some((status, false)) if status == "active" => {}
        _ => return Err(rejected("roulette.blocked", "Аккаунт заблокирован")),
    }
    let (config, version, saved, _) = load(&mut tx).await?;
    if !saved {
        return Err(rejected(
            "roulette.not_ready",
            "Рулетка ещё не настроена. Загляните позже",
        ));
    }
    let free = free_status(&mut tx, &config, client_id, now, denied).await?;
    if free.held {
        antibot::record(
            ctx,
            client_id,
            guard.as_ref(),
            antibot::Action::RouletteDenied,
            target(),
            None,
        )
        .await?;
    }
    if free.ready && Guard::asks(guard.as_ref()) {
        tx.rollback().await?;
        antibot::record(
            ctx,
            client_id,
            guard.as_ref(),
            antibot::Action::RouletteAsked,
            target(),
            None,
        )
        .await?;
        return Err(rejected(antibot::CONFIRM_CODE, CONFIRM_TEXT));
    }
    let (source, ticket) = if free.ready {
        ("free", None)
    } else {
        match store::take_ticket(&mut tx, client_id).await? {
            Some(ticket) => ("purchase", Some(ticket)),
            None => {
                return Err(rejected("roulette.no_spins", "Вращений пока нет"));
            }
        }
    };
    let subscription = prize::live_subscription(&mut tx, client_id, prefer).await?;
    let promo_on = prize::promo_enabled(&mut tx).await?;
    let spent = store::spent(&mut tx, now - config.budget.period.span()).await?;
    let weights: Vec<(usize, u32)> = config
        .sectors
        .iter()
        .enumerate()
        .filter(|(_, s)| available(s, subscription.is_some(), promo_on, &config.budget, &spent))
        .map(|(i, s)| (i, s.weight))
        .collect();
    let roll: u64 = rand::rngs::OsRng.r#gen();
    let index = match pick(&weights, roll) {
        Some(index) => index,
        None => {
            return Err(rejected(
                "roulette.empty",
                "Призы на сегодня закончились. Загляните позже",
            ));
        }
    };
    let sector = &config.sectors[index];
    let currency = installation_currency(&mut tx).await?;
    let label = sector.stored_title();
    let (balance_minor, days, promo, state) = match &sector.prize {
        Prize::Empty => (0, 0, 0, "empty"),
        Prize::Balance { amount } => {
            prize::check_currency(&sector.prize, currency)?;
            (amount.minor, 0, 0, "issued")
        }
        Prize::Days { days } => (0, i32::try_from(*days).unwrap_or(i32::MAX), 0, "issuing"),
        Prize::Promo { .. } => (0, 0, 1, "issued"),
        Prize::Custom { .. } => (0, 0, 0, "empty"),
    };
    let spin_id = store::insert_spin(
        &mut tx,
        &store::NewSpin {
            client_id,
            request_id,
            source,
            ticket_id: ticket,
            config_version: version,
            sector: i32::try_from(index).unwrap_or(0),
            prize: serde_json::to_value(&sector.prize)
                .map_err(|e| ModuleError::Core(e.to_string()))?,
            label: &label,
            balance_minor,
            days,
            promo,
            state,
            subscription_id: match &sector.prize {
                Prize::Days { .. } => subscription.map(|s| s.id.0),
                _ => None,
            },
        },
    )
    .await?;
    if let Some(ticket) = ticket {
        store::use_ticket(&mut tx, ticket, now).await?;
    }
    issue_in_tx(&mut tx, spin_id, client_id, &sector.prize, now).await?;
    tx.commit().await?;
    if let (Prize::Days { days }, Some(sub)) = (&sector.prize, subscription) {
        let applied = ctx.core.add_days(sub.id, *days, "рулетка").await;
        let mut conn = ctx.store.conn().await?;
        match applied {
            Ok(()) => {
                store::set_spin(
                    &mut conn,
                    spin_id,
                    "issued",
                    Some("Дни уже добавлены к подписке"),
                    None,
                    None,
                    Some(now),
                )
                .await?;
            }
            Err(err) => {
                store::set_spin(
                    &mut conn,
                    spin_id,
                    "failed",
                    None,
                    None,
                    Some(&format!("Дни не добавлены: {err}")),
                    None,
                )
                .await?;
            }
        }
    }
    if source == "free" && Guard::noted(guard.as_ref()) {
        antibot::record(
            ctx,
            client_id,
            guard.as_ref(),
            antibot::Action::RouletteNoted,
            serde_json::json!({
                "target": Target::Roulette.as_str(),
                "spin_id": spin_id,
                "prize": label,
            }),
            Some(spin_id),
        )
        .await?;
    }
    let mut conn = ctx.store.conn().await?;
    let row = store::spin(&mut conn, spin_id)
        .await?
        .ok_or(ModuleError::NotFound("вращение"))?;
    let (state, _) = client_state(&mut conn, client_id, denied).await?;
    Ok(SpinResult {
        spin: history_item(row),
        state,
    })
}

async fn issue_in_tx(
    tx: &mut Tx,
    spin_id: i64,
    client_id: ClientId,
    prize: &Prize,
    now: DateTime<Utc>,
) -> Result<(), ModuleError> {
    match prize {
        Prize::Balance { amount } => {
            prize::credit(
                tx,
                client_id,
                *amount,
                LedgerKind::Roulette,
                format!("roulette:{spin_id}"),
                "Приз рулетки".into(),
            )
            .await?;
            store::set_spin(
                tx,
                spin_id,
                "issued",
                Some("Деньги уже на балансе"),
                None,
                None,
                Some(now),
            )
            .await?;
        }
        Prize::Promo {
            percent,
            valid_days,
        } => {
            let until = prize::promo_until(now, *valid_days);
            let code = prize::personal_code(
                tx,
                PromoKind::Percent { percent: *percent },
                Some(until),
                &format!("Приз рулетки, клиент #{}", client_id.0),
                client_id,
            )
            .await?;
            store::set_spin(
                tx,
                spin_id,
                "issued",
                Some(&format!("Промокод действует {}", format::days(*valid_days))),
                Some(&code),
                None,
                Some(now),
            )
            .await?;
        }
        Prize::Days { .. } | Prize::Empty | Prize::Custom { .. } => {}
    }
    Ok(())
}

pub async fn grant_for_order(tx: &mut Tx, order: &Order) -> Result<bool, ModuleError> {
    if !matches!(
        order.kind,
        OrderKind::New | OrderKind::Renewal | OrderKind::Change | OrderKind::Module
    ) || order.quote.total.minor <= 0
    {
        return Ok(false);
    }
    let Some((value, _, _)) = store::config(tx).await? else {
        return Ok(false);
    };
    let Ok(config) = serde_json::from_value::<Config>(value) else {
        return Ok(false);
    };
    if !config.per_purchase
        || config
            .purchase_min
            .is_some_and(|min| order.quote.total.minor < min.minor)
    {
        return Ok(false);
    }
    Ok(store::grant_ticket(tx, order.client_id, order.id).await?)
}

pub struct RouletteModule;

#[async_trait]
impl Module for RouletteModule {
    fn id(&self) -> ModuleId {
        ModuleId::Roulette
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::admin(ctx))
    }

    fn client_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(routes::client(ctx))
    }

    async fn on_order_paid(
        &self,
        _ctx: &ModuleContext,
        tx: &mut Tx,
        order: &Order,
        _payment: &Payment,
    ) -> Result<(), ModuleError> {
        grant_for_order(tx, order).await.map(|_| ())
    }

    async fn on_event(&self, ctx: &ModuleContext, event: &DomainEvent) -> Result<(), ModuleError> {
        if let DomainEvent::PaymentRefunded { order_id, .. } = event {
            let mut conn = ctx.store.conn().await?;
            let revoked = store::revoke_ticket(&mut conn, *order_id).await?;
            if revoked > 0 {
                tracing::info!(order = %order_id, "рулетка: вращение за возвращённую покупку отозвано");
            }
        }
        Ok(())
    }
}
