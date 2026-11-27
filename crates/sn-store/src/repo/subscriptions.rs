use super::{non_negative_u32, non_negative_u64, to_i32, to_i64};
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Days, Duration, FixedOffset, NaiveDate, NaiveTime, Offset, Utc};
use sn_domain::ids::{ClientId, PeriodId, RemnaUserId, SubscriptionId, TariffId};
use sn_domain::money::{Currency, Stars};
use sn_domain::subscription::{
    AutoRenewState, EXPIRING_SOON_DAYS, ExtraTraffic, RemnaLink, RemnaMirror, Subscription,
    SubscriptionKind, SubscriptionStatus,
};
use sn_domain::traffic::TrafficStrategy;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSubscription {
    pub client_id: ClientId,
    pub tariff_id: Option<TariffId>,
    pub kind: SubscriptionKind,
    pub extra_devices: u32,
    pub autorenew: AutoRenewState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseApplied {
    pub tariff_id: Option<TariffId>,
    pub kind: SubscriptionKind,
    pub extra_devices: u32,
    pub period_started_at: DateTime<Utc>,
    pub period_days: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DashboardStats {
    pub active_paid: i64,
    pub trials: i64,
    pub expiring_soon: i64,
    pub never_connected: i64,
    pub limited: i64,
    pub revenue_today_minor: i64,
    pub revenue_7d_minor: i64,
    pub revenue_30d_minor: i64,
    pub revenue_yesterday_minor: i64,
    pub revenue_prev_7d_minor: i64,
    pub revenue_today_stars: Stars,
    pub revenue_7d_stars: Stars,
    pub revenue_30d_stars: Stars,
    pub payments_today: i64,
    pub new_clients_today: i64,
    pub new_clients_paid_today: i64,
    pub revenue_by_day: Vec<DayRevenue>,
    pub clients_by_day: Vec<DayClients>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DayRevenue {
    pub day: NaiveDate,
    pub minor: i64,
    pub stars: Stars,
    pub payments: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DayClients {
    pub day: NaiveDate,
    pub new: i64,
    pub paid: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueCursor {
    pub expire_at: DateTime<Utc>,
    pub id: SubscriptionId,
}

impl DueCursor {
    pub fn of(subscription: &Subscription) -> Option<Self> {
        subscription.mirror.expire_at.map(|expire_at| DueCursor {
            expire_at,
            id: subscription.id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndedSubscription {
    pub id: SubscriptionId,
    pub client_id: ClientId,
    pub kind: SubscriptionKind,
    pub expire_at: Option<DateTime<Utc>>,
}

pub const REVENUE_DAYS: i64 = 14;

pub(crate) const PAID_LIKE: &str = "('paid', 'partially_refunded', 'refunded')";

pub(crate) fn effective_status(alias: &str, now: &str) -> String {
    format!(
        "COALESCE({alias}.status, CASE WHEN {alias}.expire_at IS NULL THEN NULL \
         WHEN {alias}.expire_at <= {now} THEN 'expired' ELSE 'active' END)"
    )
}

fn day_start(day: NaiveDate, offset: FixedOffset) -> StoreResult<DateTime<Utc>> {
    day.and_time(NaiveTime::MIN)
        .and_utc()
        .checked_sub_signed(Duration::seconds(i64::from(offset.local_minus_utc())))
        .ok_or_else(|| StoreError::Invalid("дата вне допустимого диапазона".into()))
}

#[derive(sqlx::FromRow)]
struct CountsRow {
    active_paid: i64,
    trials: i64,
    expiring_soon: i64,
    never_connected: i64,
    limited: i64,
}

#[derive(sqlx::FromRow)]
struct RevenueRow {
    today: i64,
    yesterday: i64,
    week: i64,
    prev_week: i64,
    month: i64,
    today_stars: i64,
    week_stars: i64,
    month_stars: i64,
    payments_today: i64,
}

#[derive(sqlx::FromRow)]
struct SubscriptionRow {
    id: i64,
    public_id: Uuid,
    client_id: i64,
    tariff_id: Option<i64>,
    kind: String,
    remna_user_id: Option<i64>,
    remna_username: Option<String>,
    short_uuid: Option<String>,
    extra_devices: i32,
    period_started_at: Option<DateTime<Utc>>,
    period_days: Option<i32>,
    autorenew_enabled: bool,
    autorenew_period_id: Option<i64>,
    status: Option<String>,
    expire_at: Option<DateTime<Utc>>,
    traffic_used_bytes: Option<i64>,
    traffic_limit_bytes: Option<i64>,
    traffic_strategy: Option<String>,
    hwid_limit: Option<i32>,
    devices_count: Option<i32>,
    online_at: Option<DateTime<Utc>>,
    first_connected_at: Option<DateTime<Utc>>,
    last_node_uuid: Option<Uuid>,
    remna_updated_at: Option<DateTime<Utc>>,
    synced_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    device_limit: Option<i32>,
}

const COLUMNS: &str = "id, public_id, client_id, tariff_id, kind, remna_user_id, remna_username, short_uuid, extra_devices, \
                       period_started_at, period_days, autorenew_enabled, autorenew_period_id, status, expire_at, \
                       traffic_used_bytes, traffic_limit_bytes, traffic_strategy, hwid_limit, devices_count, online_at, \
                       first_connected_at, last_node_uuid, remna_updated_at, synced_at, created_at, ended_at, device_limit";

impl TryFrom<SubscriptionRow> for Subscription {
    type Error = StoreError;

    fn try_from(row: SubscriptionRow) -> Result<Self, Self::Error> {
        let remna = match (row.remna_user_id, row.remna_username) {
            (Some(user_id), Some(username)) => Some(RemnaLink {
                user_id: RemnaUserId(user_id),
                username,
                short_uuid: row.short_uuid,
            }),
            _ => None,
        };
        Ok(Subscription {
            id: SubscriptionId(row.id),
            public_id: row.public_id,
            client_id: ClientId(row.client_id),
            tariff_id: row.tariff_id.map(TariffId),
            kind: SubscriptionKind::parse(&row.kind)?,
            remna,
            extra_devices: non_negative_u32(row.extra_devices, "extra_devices")?,
            period_started_at: row.period_started_at,
            period_days: row
                .period_days
                .map(|v| non_negative_u32(v, "period_days"))
                .transpose()?,
            autorenew: AutoRenewState {
                enabled: row.autorenew_enabled,
                period_id: row.autorenew_period_id.map(PeriodId),
            },
            mirror: RemnaMirror {
                status: row
                    .status
                    .as_deref()
                    .map(SubscriptionStatus::parse)
                    .transpose()?,
                expire_at: row.expire_at,
                traffic_used_bytes: row
                    .traffic_used_bytes
                    .map(|v| non_negative_u64(v, "traffic_used_bytes"))
                    .transpose()?,
                traffic_limit_bytes: row
                    .traffic_limit_bytes
                    .map(|v| non_negative_u64(v, "traffic_limit_bytes"))
                    .transpose()?,
                traffic_strategy: row
                    .traffic_strategy
                    .as_deref()
                    .map(TrafficStrategy::parse_remna)
                    .transpose()?,
                hwid_limit: row
                    .hwid_limit
                    .map(|v| non_negative_u32(v, "hwid_limit"))
                    .transpose()?,
                devices_count: row
                    .devices_count
                    .map(|v| non_negative_u32(v, "devices_count"))
                    .transpose()?,
                online_at: row.online_at,
                first_connected_at: row.first_connected_at,
                last_node: row.last_node_uuid,
                remna_updated_at: row.remna_updated_at,
                synced_at: row.synced_at,
            },
            created_at: row.created_at,
            ended_at: row.ended_at,
            device_limit: row
                .device_limit
                .map(|v| non_negative_u32(v, "device_limit"))
                .transpose()?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ExtraTrafficRow {
    bytes: i64,
    window_started_at: Option<DateTime<Utc>>,
    window_lifetime: Option<i64>,
    window_ends_at: Option<DateTime<Utc>>,
}

pub struct SubscriptionsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> SubscriptionsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        SubscriptionsRepo { conn }
    }

    pub async fn create(&mut self, sub: &NewSubscription) -> StoreResult<Subscription> {
        let row: SubscriptionRow = sqlx::query_as(&format!(
            "INSERT INTO subscriptions (client_id, tariff_id, kind, extra_devices, autorenew_enabled, autorenew_period_id) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {COLUMNS}"
        ))
        .bind(sub.client_id.0)
        .bind(sub.tariff_id.map(|t| t.0))
        .bind(sub.kind.as_str())
        .bind(to_i32(sub.extra_devices, "extra_devices")?)
        .bind(sub.autorenew.enabled)
        .bind(sub.autorenew.period_id.map(|p| p.0))
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    async fn find(&mut self, sql: &str, bind: i64) -> StoreResult<Option<Subscription>> {
        let row: Option<SubscriptionRow> = sqlx::query_as(sql)
            .bind(bind)
            .fetch_optional(&mut *self.conn)
            .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn get(&mut self, id: SubscriptionId) -> StoreResult<Subscription> {
        self.find(
            &format!("SELECT {COLUMNS} FROM subscriptions WHERE id = $1"),
            id.0,
        )
        .await?
        .ok_or(StoreError::NotFound("подписка"))
    }

    pub async fn lock(&mut self, id: SubscriptionId) -> StoreResult<Subscription> {
        self.find(
            &format!("SELECT {COLUMNS} FROM subscriptions WHERE id = $1 FOR UPDATE"),
            id.0,
        )
        .await?
        .ok_or(StoreError::NotFound("подписка"))
    }

    pub async fn by_remna_user(
        &mut self,
        user_id: RemnaUserId,
    ) -> StoreResult<Option<Subscription>> {
        self.find(
            &format!("SELECT {COLUMNS} FROM subscriptions WHERE remna_user_id = $1"),
            user_id.0,
        )
        .await
    }

    pub async fn by_public_id(&mut self, public_id: Uuid) -> StoreResult<Subscription> {
        let row: Option<SubscriptionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM subscriptions WHERE public_id = $1"
        ))
        .bind(public_id)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into)
            .transpose()?
            .ok_or(StoreError::NotFound("подписка"))
    }

    pub async fn by_remna_username(&mut self, username: &str) -> StoreResult<Option<Subscription>> {
        let row: Option<SubscriptionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM subscriptions WHERE remna_username = $1"
        ))
        .bind(username)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn by_client(&mut self, client_id: ClientId) -> StoreResult<Vec<Subscription>> {
        let rows: Vec<SubscriptionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM subscriptions WHERE client_id = $1 ORDER BY ended_at IS NOT NULL, id"
        ))
        .bind(client_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn linked_for_tariff(
        &mut self,
        tariff_id: TariffId,
    ) -> StoreResult<Vec<(RemnaUserId, u32, Option<u32>)>> {
        let rows: Vec<(i64, i32, Option<i32>)> = sqlx::query_as(
            "SELECT remna_user_id, extra_devices, device_limit FROM subscriptions WHERE tariff_id = $1 \
             AND ended_at IS NULL AND remna_user_id IS NOT NULL AND remna_missing_at IS NULL \
             AND kind <> 'trial' ORDER BY id",
        )
        .bind(tariff_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(user, extra, manual)| {
                (
                    RemnaUserId(user),
                    u32::try_from(extra).unwrap_or(0),
                    manual.and_then(|v| u32::try_from(v).ok()),
                )
            })
            .collect())
    }

    pub async fn reserve_username(
        &mut self,
        id: SubscriptionId,
        username: &str,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE subscriptions SET remna_username = $2 WHERE id = $1 AND remna_username IS NULL",
        )
        .bind(id.0)
        .bind(username)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn reserved_username(&mut self, id: SubscriptionId) -> StoreResult<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT remna_username FROM subscriptions WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?
                .flatten(),
        )
    }

    pub async fn taken_usernames(&mut self, names: &[String]) -> StoreResult<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT remna_username FROM subscriptions WHERE remna_username = ANY($1)",
        )
        .bind(names)
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn rename_reserved(
        &mut self,
        id: SubscriptionId,
        from: &str,
        to: &str,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE subscriptions SET remna_username = $3 \
             WHERE id = $1 AND remna_user_id IS NULL AND remna_username = $2",
        )
        .bind(id.0)
        .bind(from)
        .bind(to)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn ordinal(
        &mut self,
        client_id: ClientId,
        before: Option<SubscriptionId>,
    ) -> StoreResult<u32> {
        let earlier: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM subscriptions WHERE client_id = $1 AND ($2::bigint IS NULL OR id < $2)",
        )
        .bind(client_id.0)
        .bind(before.map(|id| id.0))
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(u32::try_from(earlier).unwrap_or(u32::MAX - 1) + 1)
    }

    pub async fn link_remna(&mut self, id: SubscriptionId, link: &RemnaLink) -> StoreResult<()> {
        sqlx::query(
            "UPDATE subscriptions SET remna_user_id = $2, remna_username = $3, short_uuid = $4 WHERE id = $1",
        )
        .bind(id.0)
        .bind(link.user_id.0)
        .bind(&link.username)
        .bind(&link.short_uuid)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn apply_mirror(
        &mut self,
        id: SubscriptionId,
        mirror: &RemnaMirror,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE subscriptions SET status = $2, expire_at = $3, traffic_used_bytes = $4, traffic_limit_bytes = $5, \
             traffic_strategy = $6, hwid_limit = $7, devices_count = COALESCE($8, devices_count), online_at = $9, \
             first_connected_at = $10, last_node_uuid = $11, remna_updated_at = $12, synced_at = COALESCE($13, now()) \
             WHERE id = $1 AND ($12::timestamptz IS NULL OR remna_updated_at IS NULL OR remna_updated_at <= $12)",
        )
        .bind(id.0)
        .bind(mirror.status.map(|s| s.as_str()))
        .bind(mirror.expire_at)
        .bind(mirror.traffic_used_bytes.map(|v| to_i64(v, "traffic_used_bytes")).transpose()?)
        .bind(mirror.traffic_limit_bytes.map(|v| to_i64(v, "traffic_limit_bytes")).transpose()?)
        .bind(mirror.traffic_strategy.map(|s| s.as_remna()))
        .bind(mirror.hwid_limit.map(|v| to_i32(v, "hwid_limit")).transpose()?)
        .bind(mirror.devices_count.map(|v| to_i32(v, "devices_count")).transpose()?)
        .bind(mirror.online_at)
        .bind(mirror.first_connected_at)
        .bind(mirror.last_node)
        .bind(mirror.remna_updated_at)
        .bind(mirror.synced_at)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn mark_missing(&mut self, id: SubscriptionId, missing: bool) -> StoreResult<()> {
        sqlx::query(
            "UPDATE subscriptions SET remna_missing_at = CASE WHEN $2 THEN COALESCE(remna_missing_at, now()) END, \
             synced_at = now() WHERE id = $1",
        )
        .bind(id.0)
        .bind(missing)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn missing_since(
        &mut self,
        id: SubscriptionId,
    ) -> StoreResult<Option<DateTime<Utc>>> {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT remna_missing_at FROM subscriptions WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("подписка"))
    }

    pub async fn set_devices_count(&mut self, id: SubscriptionId, count: u32) -> StoreResult<()> {
        sqlx::query("UPDATE subscriptions SET devices_count = $2 WHERE id = $1")
            .bind(id.0)
            .bind(to_i32(count, "devices_count")?)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn apply_purchase(
        &mut self,
        id: SubscriptionId,
        purchase: &PurchaseApplied,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE subscriptions SET tariff_id = $2, kind = $3, extra_devices = $4, period_started_at = $5, \
             period_days = $6, ended_at = NULL WHERE id = $1",
        )
        .bind(id.0)
        .bind(purchase.tariff_id.map(|t| t.0))
        .bind(purchase.kind.as_str())
        .bind(to_i32(purchase.extra_devices, "extra_devices")?)
        .bind(purchase.period_started_at)
        .bind(to_i32(purchase.period_days, "period_days")?)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn set_device_limit(
        &mut self,
        id: SubscriptionId,
        limit: Option<u32>,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE subscriptions SET device_limit = $2 WHERE id = $1")
            .bind(id.0)
            .bind(limit.map(|v| to_i32(v, "device_limit")).transpose()?)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_extra_devices(&mut self, id: SubscriptionId, extra: u32) -> StoreResult<()> {
        sqlx::query("UPDATE subscriptions SET extra_devices = $2 WHERE id = $1")
            .bind(id.0)
            .bind(to_i32(extra, "extra_devices")?)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn extra_traffic(&mut self, id: SubscriptionId) -> StoreResult<Option<ExtraTraffic>> {
        let row: Option<ExtraTrafficRow> = sqlx::query_as(
            "SELECT bytes, window_started_at, window_lifetime, window_ends_at \
             FROM subscription_extra_traffic WHERE subscription_id = $1",
        )
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(|row| {
            Ok(ExtraTraffic {
                subscription_id: id,
                bytes: non_negative_u64(row.bytes, "extra_traffic_bytes")?,
                window_started_at: row.window_started_at,
                window_lifetime: row
                    .window_lifetime
                    .map(|v| non_negative_u64(v, "window_lifetime"))
                    .transpose()?,
                window_ends_at: row.window_ends_at,
            })
        })
        .transpose()
    }

    pub async fn set_extra_traffic(&mut self, value: &ExtraTraffic) -> StoreResult<()> {
        if value.bytes == 0 {
            return self.clear_extra_traffic(value.subscription_id).await;
        }
        sqlx::query(
            "INSERT INTO subscription_extra_traffic \
             (subscription_id, bytes, window_started_at, window_lifetime, window_ends_at) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (subscription_id) DO UPDATE SET bytes = EXCLUDED.bytes, \
             window_started_at = EXCLUDED.window_started_at, window_lifetime = EXCLUDED.window_lifetime, \
             window_ends_at = EXCLUDED.window_ends_at, updated_at = now()",
        )
        .bind(value.subscription_id.0)
        .bind(to_i64(value.bytes, "extra_traffic_bytes")?)
        .bind(value.window_started_at)
        .bind(
            value
                .window_lifetime
                .map(|v| to_i64(v, "window_lifetime"))
                .transpose()?,
        )
        .bind(value.window_ends_at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn clear_extra_traffic(&mut self, id: SubscriptionId) -> StoreResult<()> {
        sqlx::query("DELETE FROM subscription_extra_traffic WHERE subscription_id = $1")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn extra_traffic_due(
        &mut self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<SubscriptionId>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT subscription_id FROM subscription_extra_traffic \
             WHERE window_ends_at IS NOT NULL AND window_ends_at <= $1 \
             ORDER BY window_ends_at, subscription_id LIMIT $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(ids.into_iter().map(SubscriptionId).collect())
    }

    pub async fn set_autorenew(
        &mut self,
        id: SubscriptionId,
        state: AutoRenewState,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE subscriptions SET autorenew_enabled = $2, autorenew_period_id = $3 WHERE id = $1")
            .bind(id.0)
            .bind(state.enabled)
            .bind(state.period_id.map(|p| p.0))
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn expiring_before(
        &mut self,
        before: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<Subscription>> {
        let rows: Vec<SubscriptionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM subscriptions WHERE ended_at IS NULL AND expire_at IS NOT NULL AND expire_at <= $1 \
             ORDER BY expire_at LIMIT $2"
        ))
        .bind(before)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn autorenew_due(
        &mut self,
        until: DateTime<Utc>,
        since: DateTime<Utc>,
        after: Option<DueCursor>,
        limit: i64,
    ) -> StoreResult<Vec<Subscription>> {
        let filter = "ended_at IS NULL AND autorenew_enabled AND expire_at IS NOT NULL \
                      AND expire_at <= $1 AND expire_at > $2 \
                      AND NOT EXISTS (SELECT 1 FROM provider_subscriptions ps \
                      WHERE ps.subscription_id = subscriptions.id \
                      AND ps.status IN ('active', 'cancelling'))";
        let rows: Vec<SubscriptionRow> = match after {
            Some(cursor) => sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM subscriptions WHERE {filter} AND (expire_at, id) > ($3, $4) \
                     ORDER BY expire_at, id LIMIT $5"
            ))
            .bind(until)
            .bind(since)
            .bind(cursor.expire_at)
            .bind(cursor.id.0)
            .bind(limit)
            .fetch_all(&mut *self.conn)
            .await?,
            None => sqlx::query_as(&format!(
                "SELECT {COLUMNS} FROM subscriptions WHERE {filter} ORDER BY expire_at, id LIMIT $3"
            ))
            .bind(until)
            .bind(since)
            .bind(limit)
            .fetch_all(&mut *self.conn)
            .await?,
        };
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn end_expired(
        &mut self,
        expired_before: DateTime<Utc>,
        at: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<EndedSubscription>> {
        let rows: Vec<(i64, i64, String, Option<DateTime<Utc>>)> = sqlx::query_as(
            "UPDATE subscriptions s SET ended_at = $2, autorenew_enabled = false WHERE s.id IN ( \
                 SELECT x.id FROM subscriptions x \
                 WHERE x.ended_at IS NULL AND x.expire_at IS NOT NULL AND x.expire_at < $1 \
                 AND (x.status IS NULL OR x.status IN ('expired', 'disabled')) \
                 AND NOT EXISTS (SELECT 1 FROM orders o WHERE o.subscription_id = x.id \
                     AND o.status IN ('pending', 'paid', 'review')) \
                 AND (EXISTS (SELECT 1 FROM client_identities i JOIN trial_claims t \
                         ON t.kind = i.kind AND t.value = i.value WHERE i.client_id = x.client_id) \
                     OR EXISTS (SELECT 1 FROM subscriptions y WHERE y.client_id = x.client_id AND y.id <> x.id \
                         AND y.ended_at IS NULL AND (y.expire_at IS NULL OR y.expire_at >= $1))) \
                 ORDER BY x.expire_at, x.id LIMIT $3 FOR UPDATE SKIP LOCKED) \
             RETURNING s.id, s.client_id, s.kind, s.expire_at",
        )
        .bind(expired_before)
        .bind(at)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|(id, client_id, kind, expire_at)| {
                Ok(EndedSubscription {
                    id: SubscriptionId(id),
                    client_id: ClientId(client_id),
                    kind: SubscriptionKind::parse(&kind)?,
                    expire_at,
                })
            })
            .collect()
    }

    pub async fn stale_mirrors(
        &mut self,
        synced_before: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<Subscription>> {
        let rows: Vec<SubscriptionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM subscriptions WHERE ended_at IS NULL AND remna_user_id IS NOT NULL \
             AND (synced_at IS NULL OR synced_at < $1) ORDER BY synced_at NULLS FIRST LIMIT $2"
        ))
        .bind(synced_before)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn by_ids(&mut self, ids: &[SubscriptionId]) -> StoreResult<Vec<Subscription>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<SubscriptionRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM subscriptions WHERE id = ANY($1) ORDER BY id"
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn dashboard(
        &mut self,
        now: DateTime<Utc>,
        currency: Currency,
    ) -> StoreResult<DashboardStats> {
        self.dashboard_in(now, currency, Utc.fix()).await
    }

    pub async fn dashboard_in(
        &mut self,
        now: DateTime<Utc>,
        currency: Currency,
        offset: FixedOffset,
    ) -> StoreResult<DashboardStats> {
        let today = now.with_timezone(&offset).date_naive();
        let days_back = |days: u64| {
            today
                .checked_sub_days(Days::new(days))
                .ok_or_else(|| StoreError::Invalid("дата вне допустимого диапазона".into()))
                .and_then(|day| day_start(day, offset))
        };
        let today_start = days_back(0)?;
        let yesterday_start = days_back(1)?;
        let week_start = days_back(6)?;
        let prev_week_start = days_back(13)?;
        let month_start = days_back(29)?;
        let week_ago = now
            .checked_sub_signed(Duration::days(7))
            .ok_or_else(|| StoreError::Invalid("дата вне допустимого диапазона".into()))?;
        let series_start = days_back(REVENUE_DAYS as u64 - 1)?;
        let soon = now
            .checked_add_signed(Duration::days(EXPIRING_SOON_DAYS))
            .ok_or_else(|| StoreError::Invalid("дата вне допустимого диапазона".into()))?;
        let counts: CountsRow = sqlx::query_as(&format!(
            "SELECT count(*) FILTER (WHERE est = 'active' AND kind <> 'trial') AS active_paid, \
             count(*) FILTER (WHERE est = 'active' AND kind = 'trial') AS trials, \
             count(*) FILTER (WHERE est = 'active' AND expire_at > $1 AND expire_at <= $2) AS expiring_soon, \
             count(*) FILTER (WHERE est = 'active' AND first_connected_at IS NULL) AS never_connected, \
             count(*) FILTER (WHERE est = 'limited') AS limited \
             FROM (SELECT s.kind, s.expire_at, s.first_connected_at, {est} AS est \
                   FROM subscriptions s JOIN clients c ON c.id = s.client_id \
                   WHERE s.ended_at IS NULL AND s.remna_missing_at IS NULL \
                   AND c.deleted_at IS NULL AND c.status = 'active') x",
            est = effective_status("s", "$1")
        ))
        .bind(now)
        .bind(soon)
        .fetch_one(&mut *self.conn)
        .await?;
        let revenue: RevenueRow = sqlx::query_as(&format!(
            "SELECT COALESCE(sum(net) FILTER (WHERE currency = $1 AND paid_at >= $2), 0)::bigint AS today, \
             COALESCE(sum(net) FILTER (WHERE currency = $1 AND paid_at >= $3 AND paid_at < $2), 0)::bigint AS yesterday, \
             COALESCE(sum(net) FILTER (WHERE currency = $1 AND paid_at >= $4), 0)::bigint AS week, \
             COALESCE(sum(net) FILTER (WHERE currency = $1 AND paid_at >= $6 AND paid_at < $7), 0)::bigint AS prev_week, \
             COALESCE(sum(net) FILTER (WHERE currency = $1), 0)::bigint AS month, \
             COALESCE(sum(net) FILTER (WHERE currency = 'XTR' AND paid_at >= $2), 0)::bigint AS today_stars, \
             COALESCE(sum(net) FILTER (WHERE currency = 'XTR' AND paid_at >= $4), 0)::bigint AS week_stars, \
             COALESCE(sum(net) FILTER (WHERE currency = 'XTR'), 0)::bigint AS month_stars, \
             count(*) FILTER (WHERE currency = $1 AND paid_at >= $2 AND net > 0) AS payments_today \
             FROM (SELECT currency, paid_at, amount_minor - refunded_minor AS net FROM payments \
                   WHERE paid_at >= $5 AND status IN {PAID_LIKE} AND provider <> 'balance') p"
        ))
        .bind(currency.code())
        .bind(today_start)
        .bind(yesterday_start)
        .bind(week_start)
        .bind(month_start)
        .bind(prev_week_start)
        .bind(week_ago)
        .fetch_one(&mut *self.conn)
        .await?;
        let shift = f64::from(offset.local_minus_utc());
        let by_day: Vec<(NaiveDate, i64, i64, i64)> = sqlx::query_as(&format!(
            "SELECT ((LEAST(paid_at, $4) AT TIME ZONE 'UTC') + make_interval(secs => $2))::date AS day, \
             COALESCE(sum(amount_minor - refunded_minor) FILTER (WHERE currency = $1), 0)::bigint, \
             COALESCE(sum(amount_minor - refunded_minor) FILTER (WHERE currency = 'XTR'), 0)::bigint, \
             count(*) FILTER (WHERE currency = $1 AND amount_minor - refunded_minor > 0) \
             FROM payments WHERE paid_at >= $3 AND status IN {PAID_LIKE} AND provider <> 'balance' \
             GROUP BY 1"
        ))
        .bind(currency.code())
        .bind(shift)
        .bind(series_start)
        .bind(now)
        .fetch_all(&mut *self.conn)
        .await?;
        let clients: Vec<(NaiveDate, i64, i64)> = sqlx::query_as(&format!(
            "SELECT ((LEAST(c.created_at, $3) AT TIME ZONE 'UTC') + make_interval(secs => $2))::date AS day, \
             count(*), count(p.paid) FROM clients c \
             LEFT JOIN LATERAL (SELECT true AS paid FROM payments p WHERE p.client_id = c.id \
             AND p.status IN {PAID_LIKE} AND p.provider <> 'balance' LIMIT 1) p ON true \
             WHERE c.deleted_at IS NULL AND c.created_at >= $1 GROUP BY 1"
        ))
        .bind(series_start)
        .bind(shift)
        .bind(now)
        .fetch_all(&mut *self.conn)
        .await?;
        let mut revenue_by_day = Vec::with_capacity(REVENUE_DAYS as usize);
        let mut clients_by_day = Vec::with_capacity(REVENUE_DAYS as usize);
        for back in (0..REVENUE_DAYS as u64).rev() {
            let Some(day) = today.checked_sub_days(Days::new(back)) else {
                continue;
            };
            let (minor, stars, payments) = by_day
                .iter()
                .find(|(d, ..)| *d == day)
                .map_or((0, 0, 0), |(_, minor, stars, payments)| {
                    (*minor, *stars, *payments)
                });
            revenue_by_day.push(DayRevenue {
                day,
                minor,
                stars: Stars(stars),
                payments,
            });
            let (new, paid) = clients
                .iter()
                .find(|(d, ..)| *d == day)
                .map_or((0, 0), |(_, new, paid)| (*new, *paid));
            clients_by_day.push(DayClients { day, new, paid });
        }
        let (new_clients_today, new_clients_paid_today) = clients_by_day
            .last()
            .filter(|d| d.day == today)
            .map_or((0, 0), |d| (d.new, d.paid));
        Ok(DashboardStats {
            active_paid: counts.active_paid,
            trials: counts.trials,
            expiring_soon: counts.expiring_soon,
            never_connected: counts.never_connected,
            limited: counts.limited,
            revenue_today_minor: revenue.today,
            revenue_7d_minor: revenue.week,
            revenue_30d_minor: revenue.month,
            revenue_yesterday_minor: revenue.yesterday,
            revenue_prev_7d_minor: revenue.prev_week,
            revenue_today_stars: Stars(revenue.today_stars),
            revenue_7d_stars: Stars(revenue.week_stars),
            revenue_30d_stars: Stars(revenue.month_stars),
            payments_today: revenue.payments_today,
            new_clients_today,
            new_clients_paid_today,
            revenue_by_day,
            clients_by_day,
        })
    }

    pub async fn end(&mut self, id: SubscriptionId, at: DateTime<Utc>) -> StoreResult<()> {
        sqlx::query(
            "UPDATE subscriptions SET ended_at = $2, autorenew_enabled = false WHERE id = $1",
        )
        .bind(id.0)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }
}
