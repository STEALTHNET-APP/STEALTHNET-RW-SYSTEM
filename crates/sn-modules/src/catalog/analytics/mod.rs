pub mod clients;
pub mod economy;
pub mod facts;
pub mod labels;
pub mod lists;
pub mod metric;
pub mod money;
pub mod payments;
pub mod query;
pub mod retention;
pub mod snapshots;
pub mod sources;
pub mod spans;
pub mod support;
pub mod tab;
pub mod tariffs;

#[cfg(test)]
mod tests;

use crate::catalog::installation_currency;
use crate::catalog::web::{Admin, WebError};
use crate::context::ModuleContext;
use crate::error::ModuleError;
use crate::module::{Module, ScheduledTask};
use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Offset, Utc};
use metric::{RangeInfo, TabReport};
use query::{Compare, Dates, Filters, Range, SourceFilter, Step, fit_step, local_day, previous};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;
use sn_domain::money::Currency;
use sn_domain::settings::keys;
use sn_store::{Db, Store, StoreResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tab::Ctx;

const REFRESH_TASK: &str = "refresh";
const NODES_TASK: &str = "nodes";
const REFRESH_EVERY_SECS: u64 = 600;
const REBUILD_HOURS: i64 = 24;

static BUILDING: AtomicBool = AtomicBool::new(false);

pub const TABS: [&str; 8] = [
    "money",
    "clients",
    "retention",
    "payments",
    "sources",
    "tariffs",
    "economy",
    "support",
];

pub async fn offset(db: &mut impl Db) -> StoreResult<FixedOffset> {
    let minutes = db
        .settings()
        .get(keys::REMNA_UTC_OFFSET)
        .await?
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .clamp(-1439, 1439);
    let seconds = i32::try_from(minutes * 60).unwrap_or(0);
    Ok(FixedOffset::east_opt(seconds).unwrap_or(Utc.fix()))
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct TabQuery {
    #[serde(default)]
    pub from: Option<NaiveDate>,
    #[serde(default)]
    pub to: Option<NaiveDate>,
    #[serde(default)]
    pub step: Option<Step>,
    #[serde(default)]
    pub compare: Option<Compare>,
    #[serde(default)]
    pub rolling: Option<bool>,
    #[serde(default)]
    pub tariff: Option<i64>,
    #[serde(default)]
    pub category: Option<i64>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
}

fn clean(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

impl TabQuery {
    pub fn filters(&self) -> Filters {
        Filters {
            tariff: self.tariff,
            category: self.category,
            channel: clean(&self.channel),
            source: self.source.as_deref().and_then(SourceFilter::parse),
            provider: clean(&self.provider),
            method: clean(&self.method),
        }
    }
}

pub fn range(query: &TabQuery, offset: FixedOffset, now: DateTime<Utc>) -> Result<Range, WebError> {
    let bad = |text: &str| WebError::rejected("analytics.period", text.to_string());
    let today = local_day(now, offset);
    let to = query.to.unwrap_or(today).min(today);
    let from = query
        .from
        .unwrap_or_else(|| to.with_day(1).unwrap_or(to))
        .min(today);
    if from > to {
        return Err(bad("Начало периода позже конца"));
    }
    let dates = Dates { from, to };
    if dates.days() > query::MAX_DAYS {
        return Err(bad("Период — не больше десяти лет"));
    }
    let step = fit_step(dates, query.step);
    Ok(Range {
        offset,
        dates,
        step,
        prev: previous(
            dates,
            query.compare.unwrap_or_default(),
            query.rolling.unwrap_or(false),
        ),
        today,
        now,
    })
}

pub async fn ready(db: &mut impl Db, currency: Currency) -> StoreResult<bool> {
    let row: Option<(String, i32)> =
        sqlx::query_as("SELECT currency, version FROM analytics_state WHERE id")
            .fetch_optional(db.conn())
            .await?;
    Ok(matches!(row, Some((code, version)) if code == currency.code() && version == spans::VERSION))
}

struct Building;

impl Drop for Building {
    fn drop(&mut self) {
        BUILDING.store(false, Ordering::SeqCst);
    }
}

pub fn build_in_background(store: Store, currency: Currency) {
    if BUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let building = Building;
    tokio::spawn(async move {
        let _building = building;
        if let Err(e) = spans::refresh(&store, currency, None).await {
            tracing::warn!(error = %e, "аналитика: первый расчёт не удался");
        }
    });
}

pub async fn report(
    ctx: &ModuleContext,
    tab: &str,
    query: &TabQuery,
    now: DateTime<Utc>,
) -> Result<TabReport, WebError> {
    let tab: &'static str = TABS
        .iter()
        .find(|t| **t == tab)
        .copied()
        .ok_or_else(|| WebError::rejected("analytics.tab", "Такого раздела нет"))?;
    let (currency, offset, names, is_ready) = {
        let mut conn = ctx.store.conn().await?;
        let currency = installation_currency(&mut conn).await?;
        let offset = offset(&mut conn).await?;
        let names = labels::names(&mut conn).await?;
        let is_ready = ready(&mut conn, currency).await?;
        (currency, offset, names, is_ready)
    };
    let range = range(query, offset, now)?;
    if !is_ready {
        build_in_background(ctx.store.clone(), currency);
        return Ok(TabReport {
            tab,
            currency,
            range: RangeInfo::of(&range),
            groups: Vec::new(),
            notes: Vec::new(),
            preparing: true,
        });
    }
    spans::refresh_if_stale(&ctx.store, currency).await?;
    let c = Ctx {
        store: &ctx.store,
        range,
        filters: query.filters(),
        currency,
        titles: ctx.core.payment_titles(),
        names,
    };
    let groups = match tab {
        "money" => money::build(&c).await?,
        "clients" => clients::build(&c).await?,
        "retention" => retention::build(&c).await?,
        "payments" => payments::build(&c).await?,
        "sources" => sources::build(&c).await?,
        "tariffs" => tariffs::build(&c).await?,
        "economy" => economy::build(&c, ctx.core.as_ref()).await?,
        _ => support::build(&c).await?,
    };
    let mut notes = Vec::new();
    if let Some(since) = channel_since(&ctx.store).await?
        && c.range.dates.from < local_day(since, offset)
        && matches!(tab, "money" | "tariffs")
    {
        notes.push(format!(
            "Канал покупки записывается с {}: у более ранних оплат он «не записан».",
            labels::date_text(local_day(since, offset))
        ));
    }
    Ok(TabReport {
        tab,
        currency,
        range: RangeInfo::of(&c.range),
        groups,
        notes,
        preparing: false,
    })
}

async fn channel_since(store: &Store) -> StoreResult<Option<DateTime<Utc>>> {
    let mut conn = store.conn().await?;
    Ok(
        sqlx::query_scalar(
            "SELECT m.installed_on FROM _sqlx_migrations m WHERE m.version = 410 AND m.success \
             AND m.installed_on > (SELECT min(f.installed_on) FROM _sqlx_migrations f WHERE f.success) + interval '1 hour'",
        )
        .fetch_optional(conn.conn())
        .await?,
    )
}

async fn db_now(ctx: &ModuleContext) -> Result<DateTime<Utc>, WebError> {
    let mut conn = ctx.store.conn().await?;
    Ok(crate::catalog::db_now(&mut conn).await?)
}

async fn tab_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Path(tab): Path<String>,
    Query(query): Query<TabQuery>,
) -> Result<Json<TabReport>, WebError> {
    admin.require(Permission::Money)?;
    let now = db_now(&ctx).await?;
    Ok(Json(report(&ctx, &tab, &query, now).await?))
}

async fn list_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<lists::ListQuery>,
) -> Result<Json<lists::ListPage>, WebError> {
    admin.require(Permission::Money)?;
    let now = db_now(&ctx).await?;
    Ok(Json(lists::page(&ctx, &query, now).await?))
}

async fn options_route(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<lists::Options>, WebError> {
    admin.require(Permission::Money)?;
    Ok(Json(lists::options(&ctx).await?))
}

pub struct AnalyticsModule;

#[async_trait]
impl Module for AnalyticsModule {
    fn id(&self) -> ModuleId {
        ModuleId::Analytics
    }

    fn admin_routes(&self, ctx: ModuleContext) -> Option<Router> {
        Some(
            Router::new()
                .route("/tab/{tab}", get(tab_route))
                .route("/list", get(list_route))
                .route("/options", get(options_route))
                .with_state(ctx),
        )
    }

    fn tasks(&self) -> Vec<ScheduledTask> {
        vec![
            ScheduledTask {
                name: REFRESH_TASK,
                every: Duration::from_secs(REFRESH_EVERY_SECS),
            },
            ScheduledTask {
                name: NODES_TASK,
                every: Duration::from_secs(REFRESH_EVERY_SECS),
            },
        ]
    }

    async fn run_task(&self, ctx: &ModuleContext, task: &str) -> Result<(), ModuleError> {
        match task {
            REFRESH_TASK => {
                let currency = {
                    let mut conn = ctx.store.conn().await?;
                    installation_currency(&mut conn).await?
                };
                let done = spans::refresh(
                    &ctx.store,
                    currency,
                    Some(chrono::Duration::hours(REBUILD_HOURS)),
                )
                .await?;
                if done.full {
                    tracing::info!(
                        clients = done.clients,
                        "аналитика: предрасчёт собран заново"
                    );
                }
                let mut conn = ctx.store.conn().await?;
                let offset = offset(&mut conn).await?;
                let now = crate::catalog::db_now(&mut conn).await?;
                snapshots::daily(&mut conn, local_day(now, offset), currency).await?;
                Ok(())
            }
            NODES_TASK => {
                let now = {
                    let mut conn = ctx.store.conn().await?;
                    crate::catalog::db_now(&mut conn).await?
                };
                snapshots::sample_nodes(&ctx.store, ctx.core.as_ref(), now).await
            }
            other => Err(ModuleError::Invalid(format!("у модуля нет задачи {other}"))),
        }
    }
}
