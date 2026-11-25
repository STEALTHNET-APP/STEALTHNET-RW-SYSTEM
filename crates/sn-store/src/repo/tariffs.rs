use super::{non_negative_u32, non_negative_u64, to_i32, to_i64};
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{CategoryId, ClientId, PeriodId, SquadId, SubscriptionId, TariffId};
use sn_domain::money::Stars;
use sn_domain::tariff::{
    Category, CurrencyPrices, DeviceDiscount, DevicePolicy, ExtraDevices, Tariff, TariffChannels,
    TariffPeriod, TrafficAddon,
};
use sn_domain::traffic::{TrafficPolicy, TrafficReset};
use sqlx::postgres::PgArguments;
use sqlx::query::QueryScalar;
use sqlx::types::Json;
use sqlx::{PgConnection, Postgres};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TariffHolder {
    pub client_id: ClientId,
    pub subscription_id: SubscriptionId,
    pub name: Option<String>,
    pub telegram_username: Option<String>,
    pub email: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TariffUsage {
    pub active: i64,
    pub ended: i64,
    pub holders: Vec<TariffHolder>,
}

impl TariffUsage {
    pub fn total(&self) -> i64 {
        self.active + self.ended
    }
}

#[derive(sqlx::FromRow)]
struct HolderRow {
    subscription_id: i64,
    client_id: i64,
    expire_at: Option<DateTime<Utc>>,
    active: bool,
    first_name: Option<String>,
    last_name: Option<String>,
    telegram_username: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCategory {
    pub name: String,
    pub sort: i32,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPeriod {
    pub days: u32,
    pub prices: CurrencyPrices,
    pub stars: Option<Stars>,
    pub sort: i32,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTariff {
    pub category_id: CategoryId,
    pub name: String,
    pub description: Option<String>,
    pub on_sale: bool,
    pub sort: i32,
    pub devices: DevicePolicy,
    pub traffic: TrafficPolicy,
    pub traffic_addon: Option<TrafficAddon>,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
    pub periods: Vec<NewPeriod>,
}

#[derive(sqlx::FromRow)]
struct TariffRow {
    id: i64,
    category_id: i64,
    name: String,
    description: Option<String>,
    on_sale: bool,
    sort: i32,
    devices_included: i32,
    extra_devices_enabled: bool,
    extra_devices_max: i32,
    extra_device_usd_minor: Option<i64>,
    extra_device_rub_minor: Option<i64>,
    extra_device_stars: Option<i64>,
    traffic_limit_bytes: Option<i64>,
    traffic_reset: String,
    external_squad_uuid: Option<Uuid>,
    traffic_addon: Option<Json<TrafficAddon>>,
    device_discounts: Json<Vec<DeviceDiscount>>,
}

#[derive(sqlx::FromRow, serde::Deserialize)]
struct PeriodRow {
    id: i64,
    tariff_id: i64,
    days: i32,
    price_usd_minor: Option<i64>,
    price_rub_minor: Option<i64>,
    stars: Option<i64>,
    sort: i32,
    active: bool,
}

const TARIFF_COLUMNS: &str = "id, category_id, name, description, on_sale, sort, devices_included, extra_devices_enabled, \
                              extra_devices_max, extra_device_usd_minor, extra_device_rub_minor, extra_device_stars, \
                              traffic_limit_bytes, traffic_reset, external_squad_uuid, traffic_addon, \
                              device_discounts";

const PERIOD_COLUMNS: &str =
    "id, tariff_id, days, price_usd_minor, price_rub_minor, stars, sort, active";

#[derive(sqlx::FromRow)]
struct FullTariffRow {
    #[sqlx(flatten)]
    tariff: TariffRow,
    periods: Json<Vec<PeriodRow>>,
    squads: Vec<Uuid>,
}

const FULL_TARIFF: &str = "SELECT id, category_id, name, description, on_sale, sort, devices_included, \
     extra_devices_enabled, extra_devices_max, extra_device_usd_minor, extra_device_rub_minor, extra_device_stars, \
     traffic_limit_bytes, traffic_reset, external_squad_uuid, traffic_addon, device_discounts, \
     COALESCE((SELECT jsonb_agg(jsonb_build_object('id', p.id, 'tariff_id', p.tariff_id, 'days', p.days, \
         'price_usd_minor', p.price_usd_minor, 'price_rub_minor', p.price_rub_minor, 'stars', p.stars, \
         'sort', p.sort, 'active', p.active) ORDER BY p.sort, p.days) \
       FROM tariff_periods p WHERE p.tariff_id = t.id), '[]'::jsonb) AS periods, \
     COALESCE((SELECT array_agg(q.squad_uuid ORDER BY q.position, q.squad_uuid) \
       FROM tariff_squads q WHERE q.tariff_id = t.id), '{}'::uuid[]) AS squads \
     FROM tariffs t WHERE t.id = $1 AND t.deleted_at IS NULL";

fn period_from(row: PeriodRow) -> StoreResult<TariffPeriod> {
    Ok(TariffPeriod {
        id: PeriodId(row.id),
        days: non_negative_u32(row.days, "days")?,
        prices: CurrencyPrices {
            usd: row.price_usd_minor,
            rub: row.price_rub_minor,
        },
        stars: row.stars.map(Stars),
        sort: row.sort,
        active: row.active,
    })
}

fn tariff_from(
    row: TariffRow,
    periods: Vec<TariffPeriod>,
    squads: Vec<SquadId>,
) -> StoreResult<Tariff> {
    let extra = row.extra_devices_enabled.then_some(ExtraDevices {
        max: non_negative_u32(row.extra_devices_max, "extra_devices_max")?,
        monthly: CurrencyPrices {
            usd: row.extra_device_usd_minor,
            rub: row.extra_device_rub_minor,
        },
        monthly_stars: row.extra_device_stars.map(Stars),
    });
    Ok(Tariff {
        id: TariffId(row.id),
        category_id: CategoryId(row.category_id),
        name: row.name,
        description: row.description,
        on_sale: row.on_sale,
        sort: row.sort,
        periods,
        devices: DevicePolicy {
            included: non_negative_u32(row.devices_included, "devices_included")?,
            extra,
        },
        traffic: TrafficPolicy {
            limit_bytes: row
                .traffic_limit_bytes
                .map(|v| non_negative_u64(v, "traffic_limit_bytes"))
                .transpose()?,
            reset: TrafficReset::parse(&row.traffic_reset)?,
        },
        traffic_addon: row.traffic_addon.map(|addon| addon.0),
        squads,
        external_squad: row.external_squad_uuid.map(SquadId),
        device_discounts: row.device_discounts.0,
    })
}

type CategoryRow = (i64, String, i32, bool, Option<i32>);

const CATEGORY_COLUMNS: &str = "id, name, sort, visible, subscription_limit";

fn category_from(row: CategoryRow) -> StoreResult<Category> {
    let (id, name, sort, visible, limit) = row;
    Ok(Category {
        id: CategoryId(id),
        name,
        sort,
        visible,
        subscription_limit: limit
            .map(|v| non_negative_u32(v, "subscription_limit"))
            .transpose()?,
    })
}

type TariffQuery<'q> = QueryScalar<'q, Postgres, i64, PgArguments>;

fn with_tariff<'q>(query: TariffQuery<'q>, tariff: &'q NewTariff) -> StoreResult<TariffQuery<'q>> {
    let extra = tariff.devices.extra.as_ref();
    Ok(query
        .bind(tariff.category_id.0)
        .bind(tariff.name.trim())
        .bind(&tariff.description)
        .bind(tariff.on_sale)
        .bind(tariff.sort)
        .bind(to_i32(tariff.devices.included, "devices_included")?)
        .bind(extra.is_some())
        .bind(to_i32(extra.map_or(0, |e| e.max), "extra_devices_max")?)
        .bind(extra.and_then(|e| e.monthly.usd))
        .bind(extra.and_then(|e| e.monthly.rub))
        .bind(extra.and_then(|e| e.monthly_stars).map(|s| s.0))
        .bind(
            tariff
                .traffic
                .limit_bytes
                .map(|v| to_i64(v, "traffic_limit_bytes"))
                .transpose()?,
        )
        .bind(tariff.traffic.reset.as_str())
        .bind(tariff.external_squad.map(|s| s.0))
        .bind(tariff.traffic_addon.clone().map(Json)))
}

fn check_periods(periods: &[NewPeriod]) -> StoreResult<()> {
    let mut seen = HashSet::new();
    for period in periods {
        if !seen.insert(period.days) {
            return Err(StoreError::Invalid(format!(
                "срок {} дн. указан дважды",
                period.days
            )));
        }
    }
    Ok(())
}

async fn upsert_periods(
    conn: &mut PgConnection,
    tariff_id: i64,
    periods: &[NewPeriod],
) -> StoreResult<()> {
    for period in periods {
        sqlx::query(
            "INSERT INTO tariff_periods (tariff_id, days, price_usd_minor, price_rub_minor, stars, sort, active) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (tariff_id, days) DO UPDATE SET price_usd_minor = EXCLUDED.price_usd_minor, \
             price_rub_minor = EXCLUDED.price_rub_minor, stars = EXCLUDED.stars, sort = EXCLUDED.sort, \
             active = EXCLUDED.active",
        )
        .bind(tariff_id)
        .bind(to_i32(period.days, "days")?)
        .bind(period.prices.usd)
        .bind(period.prices.rub)
        .bind(period.stars.map(|s| s.0))
        .bind(period.sort)
        .bind(period.active)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn replace_squads(
    conn: &mut PgConnection,
    tariff_id: i64,
    squads: &[SquadId],
) -> StoreResult<()> {
    let mut seen = HashSet::new();
    let ordered: Vec<Uuid> = squads
        .iter()
        .filter(|s| seen.insert(s.0))
        .map(|s| s.0)
        .collect();
    sqlx::query("DELETE FROM tariff_squads WHERE tariff_id = $1")
        .bind(tariff_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO tariff_squads (tariff_id, squad_uuid, position) \
         SELECT $1, squad, (n - 1)::int FROM unnest($2::uuid[]) WITH ORDINALITY AS s(squad, n)",
    )
    .bind(tariff_id)
    .bind(&ordered)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub struct TariffsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> TariffsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        TariffsRepo { conn }
    }

    pub async fn categories(&mut self, include_hidden: bool) -> StoreResult<Vec<Category>> {
        let rows: Vec<CategoryRow> = sqlx::query_as(&format!(
            "SELECT {CATEGORY_COLUMNS} FROM tariff_categories \
             WHERE deleted_at IS NULL AND ($1 OR visible) ORDER BY sort, id"
        ))
        .bind(include_hidden)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(category_from).collect()
    }

    pub async fn category(&mut self, id: CategoryId) -> StoreResult<Category> {
        let row: Option<CategoryRow> = sqlx::query_as(&format!(
            "SELECT {CATEGORY_COLUMNS} FROM tariff_categories WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        category_from(row.ok_or(StoreError::NotFound("категория"))?)
    }

    pub async fn create_category(&mut self, category: &NewCategory) -> StoreResult<Category> {
        let row: CategoryRow = sqlx::query_as(&format!(
            "INSERT INTO tariff_categories (name, sort, visible) VALUES ($1, $2, $3) \
             RETURNING {CATEGORY_COLUMNS}"
        ))
        .bind(category.name.trim())
        .bind(category.sort)
        .bind(category.visible)
        .fetch_one(&mut *self.conn)
        .await?;
        category_from(row)
    }

    pub async fn update_category(
        &mut self,
        id: CategoryId,
        category: &NewCategory,
    ) -> StoreResult<Category> {
        let row: Option<CategoryRow> = sqlx::query_as(&format!(
            "UPDATE tariff_categories SET name = $2, sort = $3, visible = $4 \
             WHERE id = $1 AND deleted_at IS NULL RETURNING {CATEGORY_COLUMNS}"
        ))
        .bind(id.0)
        .bind(category.name.trim())
        .bind(category.sort)
        .bind(category.visible)
        .fetch_optional(&mut *self.conn)
        .await?;
        category_from(row.ok_or(StoreError::NotFound("категория"))?)
    }

    pub async fn set_subscription_limit(
        &mut self,
        id: CategoryId,
        limit: Option<u32>,
    ) -> StoreResult<Category> {
        let row: Option<CategoryRow> = sqlx::query_as(&format!(
            "UPDATE tariff_categories SET subscription_limit = $2 \
             WHERE id = $1 AND deleted_at IS NULL RETURNING {CATEGORY_COLUMNS}"
        ))
        .bind(id.0)
        .bind(limit.map(|v| to_i32(v, "subscription_limit")).transpose()?)
        .fetch_optional(&mut *self.conn)
        .await?;
        category_from(row.ok_or(StoreError::NotFound("категория"))?)
    }

    pub async fn set_device_discounts(
        &mut self,
        id: TariffId,
        discounts: &[DeviceDiscount],
    ) -> StoreResult<()> {
        let done = sqlx::query(
            "UPDATE tariffs SET device_discounts = $2 WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.0)
        .bind(Json(discounts))
        .execute(&mut *self.conn)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("тариф"));
        }
        Ok(())
    }

    async fn assemble(&mut self, rows: Vec<TariffRow>) -> StoreResult<Vec<Tariff>> {
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let periods: Vec<PeriodRow> = sqlx::query_as(&format!(
            "SELECT {PERIOD_COLUMNS} FROM tariff_periods WHERE tariff_id = ANY($1) ORDER BY sort, days"
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        let squads: Vec<(i64, Uuid)> = sqlx::query_as(
            "SELECT tariff_id, squad_uuid FROM tariff_squads WHERE tariff_id = ANY($1) ORDER BY position, squad_uuid",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        let mut periods_by: HashMap<i64, Vec<TariffPeriod>> = HashMap::new();
        for row in periods {
            let tariff_id = row.tariff_id;
            periods_by
                .entry(tariff_id)
                .or_default()
                .push(period_from(row)?);
        }
        let mut squads_by: HashMap<i64, Vec<SquadId>> = HashMap::new();
        for (tariff_id, squad) in squads {
            squads_by.entry(tariff_id).or_default().push(SquadId(squad));
        }
        rows.into_iter()
            .map(|row| {
                let id = row.id;
                tariff_from(
                    row,
                    periods_by.remove(&id).unwrap_or_default(),
                    squads_by.remove(&id).unwrap_or_default(),
                )
            })
            .collect()
    }

    pub async fn list(&mut self, include_off_sale: bool) -> StoreResult<Vec<Tariff>> {
        let rows: Vec<TariffRow> = sqlx::query_as(&format!(
            "SELECT {TARIFF_COLUMNS} FROM tariffs WHERE deleted_at IS NULL AND ($1 OR on_sale) ORDER BY sort, id"
        ))
        .bind(include_off_sale)
        .fetch_all(&mut *self.conn)
        .await?;
        self.assemble(rows).await
    }

    pub async fn get(&mut self, id: TariffId) -> StoreResult<Tariff> {
        let row: FullTariffRow = sqlx::query_as(FULL_TARIFF)
            .bind(id.0)
            .fetch_optional(&mut *self.conn)
            .await?
            .ok_or(StoreError::NotFound("тариф"))?;
        let periods = row
            .periods
            .0
            .into_iter()
            .map(period_from)
            .collect::<StoreResult<Vec<_>>>()?;
        let squads = row.squads.into_iter().map(SquadId).collect();
        tariff_from(row.tariff, periods, squads)
    }

    pub async fn create(&mut self, tariff: &NewTariff) -> StoreResult<Tariff> {
        check_periods(&tariff.periods)?;
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        let id: i64 = with_tariff(
            sqlx::query_scalar(
                "INSERT INTO tariffs (category_id, name, description, on_sale, sort, devices_included, \
                 extra_devices_enabled, extra_devices_max, extra_device_usd_minor, extra_device_rub_minor, \
                 extra_device_stars, traffic_limit_bytes, traffic_reset, external_squad_uuid, traffic_addon) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) RETURNING id",
            ),
            tariff,
        )?
        .fetch_one(&mut *tx)
        .await?;
        upsert_periods(&mut tx, id, &tariff.periods).await?;
        replace_squads(&mut tx, id, &tariff.squads).await?;
        tx.commit().await?;
        self.get(TariffId(id)).await
    }

    pub async fn update(&mut self, id: TariffId, tariff: &NewTariff) -> StoreResult<Tariff> {
        check_periods(&tariff.periods)?;
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        with_tariff(
            sqlx::query_scalar(
                "UPDATE tariffs SET category_id = $1, name = $2, description = $3, on_sale = $4, sort = $5, \
                 devices_included = $6, extra_devices_enabled = $7, extra_devices_max = $8, \
                 extra_device_usd_minor = $9, extra_device_rub_minor = $10, extra_device_stars = $11, \
                 traffic_limit_bytes = $12, traffic_reset = $13, external_squad_uuid = $14, \
                 traffic_addon = $15 WHERE id = $16 AND deleted_at IS NULL RETURNING id",
            ),
            tariff,
        )?
        .bind(id.0)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound("тариф"))?;
        upsert_periods(&mut tx, id.0, &tariff.periods).await?;
        let kept = tariff
            .periods
            .iter()
            .map(|p| to_i32(p.days, "days"))
            .collect::<StoreResult<Vec<i32>>>()?;
        sqlx::query(
            "UPDATE tariff_periods SET active = false WHERE tariff_id = $1 AND active AND NOT (days = ANY($2))",
        )
        .bind(id.0)
        .bind(&kept)
        .execute(&mut *tx)
        .await?;
        replace_squads(&mut tx, id.0, &tariff.squads).await?;
        tx.commit().await?;
        self.get(id).await
    }

    pub async fn set_on_sale(&mut self, id: TariffId, on_sale: bool) -> StoreResult<()> {
        let done =
            sqlx::query("UPDATE tariffs SET on_sale = $2 WHERE id = $1 AND deleted_at IS NULL")
                .bind(id.0)
                .bind(on_sale)
                .execute(&mut *self.conn)
                .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("тариф"));
        }
        Ok(())
    }

    pub async fn channels(
        &mut self,
        ids: &[TariffId],
    ) -> StoreResult<HashMap<TariffId, TariffChannels>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<(i64, bool, bool, bool)> = sqlx::query_as(
            "SELECT tariff_id, bot, cabinet, miniapp FROM tariff_channels WHERE tariff_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        let mut out: HashMap<TariffId, TariffChannels> = ids
            .into_iter()
            .map(|id| (TariffId(id), TariffChannels::default()))
            .collect();
        for (id, bot, cabinet, miniapp) in rows {
            out.insert(
                TariffId(id),
                TariffChannels {
                    bot,
                    cabinet,
                    miniapp,
                },
            );
        }
        Ok(out)
    }

    pub async fn set_channels(
        &mut self,
        id: TariffId,
        channels: TariffChannels,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO tariff_channels (tariff_id, bot, cabinet, miniapp) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (tariff_id) DO UPDATE SET bot = $2, cabinet = $3, miniapp = $4, updated_at = now()",
        )
        .bind(id.0)
        .bind(channels.bot)
        .bind(channels.cabinet)
        .bind(channels.miniapp)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn sales_since(&mut self, since: DateTime<Utc>) -> StoreResult<Vec<(TariffId, i64)>> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT (snapshot ->> 'tariff_id')::bigint AS tariff, count(*) FROM orders \
             WHERE kind IN ('new', 'renewal', 'change') AND status IN ('paid', 'fulfilled') \
             AND snapshot IS NOT NULL AND COALESCE(paid_at, created_at) >= $1 \
             GROUP BY 1",
        )
        .bind(since)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows.into_iter().map(|(id, n)| (TariffId(id), n)).collect())
    }

    pub async fn usage(
        &mut self,
        id: TariffId,
        now: DateTime<Utc>,
        shown: i64,
    ) -> StoreResult<TariffUsage> {
        let (active, ended): (i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE expire_at IS NULL OR expire_at > $2), \
             count(*) FILTER (WHERE expire_at <= $2) \
             FROM subscriptions WHERE tariff_id = $1 AND ended_at IS NULL",
        )
        .bind(id.0)
        .bind(now)
        .fetch_one(&mut *self.conn)
        .await?;
        let rows: Vec<HolderRow> = if shown > 0 && active + ended > 0 {
            sqlx::query_as(
                "SELECT s.id AS subscription_id, s.client_id, s.expire_at, \
                 (s.expire_at IS NULL OR s.expire_at > $2) AS active, \
                 tg.meta ->> 'first_name' AS first_name, tg.meta ->> 'last_name' AS last_name, \
                 tg.meta ->> 'username' AS telegram_username, em.value AS email \
                 FROM subscriptions s \
                 LEFT JOIN LATERAL (SELECT i.meta FROM client_identities i WHERE i.client_id = s.client_id \
                     AND i.kind = 'telegram' ORDER BY i.id LIMIT 1) tg ON true \
                 LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = s.client_id \
                     AND i.kind = 'email' ORDER BY i.id LIMIT 1) em ON true \
                 WHERE s.tariff_id = $1 AND s.ended_at IS NULL \
                 ORDER BY (s.expire_at IS NULL OR s.expire_at > $2) DESC, s.expire_at DESC NULLS FIRST, s.id \
                 LIMIT $3",
            )
            .bind(id.0)
            .bind(now)
            .bind(shown)
            .fetch_all(&mut *self.conn)
            .await?
        } else {
            Vec::new()
        };
        Ok(TariffUsage {
            active,
            ended,
            holders: rows
                .into_iter()
                .map(|row| TariffHolder {
                    client_id: ClientId(row.client_id),
                    subscription_id: SubscriptionId(row.subscription_id),
                    name: super::clients::display_name(
                        row.first_name.as_deref(),
                        row.last_name.as_deref(),
                    ),
                    telegram_username: row.telegram_username,
                    email: row.email,
                    expire_at: row.expire_at,
                    active: row.active,
                })
                .collect(),
        })
    }

    pub async fn category_tariffs(&mut self, id: CategoryId) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM tariffs WHERE category_id = $1 AND deleted_at IS NULL",
        )
        .bind(id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn delete_category(&mut self, id: CategoryId) -> StoreResult<()> {
        let done = sqlx::query(
            "UPDATE tariff_categories SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("категория"));
        }
        Ok(())
    }

    pub async fn soft_delete(&mut self, id: TariffId) -> StoreResult<()> {
        sqlx::query("UPDATE tariffs SET deleted_at = now(), on_sale = false WHERE id = $1 AND deleted_at IS NULL")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn squads_in_use(&mut self) -> StoreResult<Vec<SquadId>> {
        let rows: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT s.squad_uuid FROM tariff_squads s JOIN tariffs t ON t.id = s.tariff_id WHERE t.deleted_at IS NULL",
        )
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows.into_iter().map(SquadId).collect())
    }
}
