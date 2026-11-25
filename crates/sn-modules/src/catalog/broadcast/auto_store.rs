use super::auto::{Delay, Localized, RuleContent, Trigger, Unit};
use super::store::{Problem, Progress};
use super::{ButtonSpec, Channel, Lane};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use sn_domain::client::Locale;
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::money::Currency;
use sn_store::{ClientConditions, Db, SqlArg, SqlArgs, StoreError, StoreResult};
use std::collections::HashMap;

const PROBLEMS: i64 = 6;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RuleRow {
    pub id: i64,
    pub trigger: String,
    pub amount: i32,
    pub unit: String,
    pub percent: Option<i16>,
    pub with_autorenew: bool,
    pub channel: String,
    pub subject: Option<String>,
    pub text: String,
    pub image_id: Option<i64>,
    pub buttons: serde_json::Value,
    pub en: Option<serde_json::Value>,
    pub audience: Option<serde_json::Value>,
    pub preset: Option<String>,
    pub enabled: bool,
    pub version: i32,
    pub notice: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, trigger, amount, unit, percent, with_autorenew, channel, subject, text, image_id, buttons, \
                       en, audience, preset, enabled, version, notice, checked_at, created_at, updated_at";

pub struct Parsed {
    pub trigger: Trigger,
    pub delay: Delay,
    pub percent: Option<u8>,
    pub channel: Channel,
    pub buttons: Vec<ButtonSpec>,
    pub en: Option<Localized>,
    pub audience: Option<ClientConditions>,
}

pub fn parse(row: &RuleRow) -> StoreResult<Parsed> {
    let trigger = Trigger::parse(&row.trigger)
        .ok_or_else(|| StoreError::Decode(format!("событие правила: {}", row.trigger)))?;
    let unit = Unit::parse(&row.unit)
        .ok_or_else(|| StoreError::Decode(format!("единица срока правила: {}", row.unit)))?;
    let channel = Channel::parse(&row.channel)
        .ok_or_else(|| StoreError::Decode(format!("канал правила: {}", row.channel)))?;
    let buttons = serde_json::from_value(row.buttons.clone())
        .map_err(|e| StoreError::Decode(format!("кнопки правила: {e}")))?;
    let en = match &row.en {
        Some(value) => Some(
            serde_json::from_value::<Localized>(value.clone())
                .map_err(|e| StoreError::Decode(format!("английский текст правила: {e}")))?,
        ),
        None => None,
    };
    let audience = match &row.audience {
        Some(value) => Some(
            serde_json::from_value::<ClientConditions>(value.clone())
                .map_err(|e| StoreError::Decode(format!("условия «Кому» правила: {e}")))?,
        ),
        None => None,
    };
    Ok(Parsed {
        trigger,
        delay: Delay {
            amount: u16::try_from(row.amount).unwrap_or_default(),
            unit,
        },
        percent: row.percent.and_then(|p| u8::try_from(p).ok()),
        channel,
        buttons,
        en,
        audience,
    })
}

fn json<T: serde::Serialize>(value: &T) -> StoreResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| StoreError::Invalid(e.to_string()))
}

fn en_json(content: &RuleContent) -> StoreResult<Option<serde_json::Value>> {
    content.en.as_ref().map(json).transpose()
}

fn audience_json(content: &RuleContent) -> StoreResult<Option<serde_json::Value>> {
    content.audience.as_ref().map(json).transpose()
}

pub async fn insert(
    db: &mut impl Db,
    content: &RuleContent,
    enabled: bool,
    preset: Option<&str>,
    admin: Option<AdminId>,
) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO auto_broadcasts (trigger, amount, unit, percent, with_autorenew, channel, subject, text, \
         image_id, buttons, en, enabled, preset, created_by, audience) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) RETURNING id",
    )
    .bind(content.trigger.as_str())
    .bind(i32::from(content.delay.amount))
    .bind(content.delay.unit.as_str())
    .bind(content.percent.map(i16::from))
    .bind(content.with_autorenew)
    .bind(content.channel.as_str())
    .bind(content.subject.as_deref())
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(en_json(content)?)
    .bind(enabled)
    .bind(preset)
    .bind(admin.map(|a| a.0))
    .bind(audience_json(content)?)
    .fetch_one(db.conn())
    .await?)
}

pub async fn update(
    db: &mut impl Db,
    id: i64,
    content: &RuleContent,
    enabled: bool,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE auto_broadcasts SET \
         version = version + CASE WHEN (channel, subject, text, image_id, buttons, en) \
             IS DISTINCT FROM ($7, $8, $9, $10, $11, $12) THEN 1 ELSE 0 END, \
         trigger = $2, amount = $3, unit = $4, percent = $5, with_autorenew = $6, channel = $7, subject = $8, \
         text = $9, image_id = $10, buttons = $11, en = $12, enabled = $13, audience = $14, \
         notice = CASE WHEN $13 THEN notice END \
         WHERE id = $1",
    )
    .bind(id)
    .bind(content.trigger.as_str())
    .bind(i32::from(content.delay.amount))
    .bind(content.delay.unit.as_str())
    .bind(content.percent.map(i16::from))
    .bind(content.with_autorenew)
    .bind(content.channel.as_str())
    .bind(content.subject.as_deref())
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(en_json(content)?)
    .bind(enabled)
    .bind(audience_json(content)?)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_enabled(db: &mut impl Db, id: i64, enabled: bool) -> StoreResult<()> {
    sqlx::query(
        "UPDATE auto_broadcasts SET enabled = $2, notice = CASE WHEN $2 THEN notice END WHERE id = $1",
    )
    .bind(id)
    .bind(enabled)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn remove(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE auto_broadcasts SET deleted_at = now(), enabled = false, notice = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn stop_queue(db: &mut impl Db, id: i64) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE broadcast_recipients r SET state = 'skipped', done_at = now(), \
         error = 'Правило выключено до отправки' \
         FROM broadcasts b WHERE b.id = r.broadcast_id AND b.rule_id = $1 AND b.status = 'sending' \
         AND r.state = 'pending'",
    )
    .bind(id)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn lock(db: &mut impl Db, id: i64) -> StoreResult<RuleRow> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {COLUMNS} FROM auto_broadcasts WHERE id = $1 AND deleted_at IS NULL FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("правило"))
}

pub async fn row(db: &mut impl Db, id: i64) -> StoreResult<RuleRow> {
    sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {COLUMNS} FROM auto_broadcasts WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("правило"))
}

pub async fn rows(db: &mut impl Db) -> StoreResult<Vec<RuleRow>> {
    Ok(sqlx::query_as::<_, RuleRow>(&format!(
        "SELECT {COLUMNS} FROM auto_broadcasts WHERE deleted_at IS NULL ORDER BY id"
    ))
    .fetch_all(db.conn())
    .await?)
}

pub async fn enabled_ids(db: &mut impl Db) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM auto_broadcasts WHERE enabled AND deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(db.conn())
    .await?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub cases: i64,
    pub unreachable: i64,
    pub progress: Progress,
    pub last_sent_at: Option<DateTime<Utc>>,
}

type CaseRow = (i64, i64, i64);
type LaneRow = (i64, i64, i64, i64, i64, i64, Option<DateTime<Utc>>);

pub async fn tallies(db: &mut impl Db, ids: &[i64]) -> StoreResult<HashMap<i64, Tally>> {
    let mut out: HashMap<i64, Tally> = HashMap::new();
    let cases: Vec<CaseRow> = sqlx::query_as(
        "SELECT rule_id, count(*), count(*) FILTER (WHERE NOT queued) FROM auto_broadcast_cases \
         WHERE rule_id = ANY($1) GROUP BY rule_id",
    )
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    for (id, total, unreachable) in cases {
        let entry = out.entry(id).or_default();
        entry.cases = total;
        entry.unreachable = unreachable;
    }
    let lanes: Vec<LaneRow> = sqlx::query_as(
        "SELECT b.rule_id, count(*) FILTER (WHERE r.state = 'sent'), count(*) FILTER (WHERE r.state = 'failed'), \
         count(*) FILTER (WHERE r.state = 'blocked'), count(*) FILTER (WHERE r.state = 'skipped'), \
         count(*) FILTER (WHERE r.state IN ('pending', 'sending')), max(r.done_at) FILTER (WHERE r.state = 'sent') \
         FROM broadcasts b JOIN broadcast_recipients r ON r.broadcast_id = b.id \
         WHERE b.rule_id = ANY($1) GROUP BY b.rule_id",
    )
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    for (id, sent, failed, blocked, skipped, pending, last) in lanes {
        let entry = out.entry(id).or_default();
        entry.progress = Progress {
            total: sent + failed + blocked + skipped + pending,
            sent,
            failed,
            blocked,
            skipped,
            pending,
        };
        entry.last_sent_at = last;
    }
    Ok(out)
}

pub async fn problems(db: &mut impl Db, id: i64) -> StoreResult<Vec<Problem>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT channel, reason, count(*) FROM ( \
             SELECT r.channel, CASE WHEN r.channel = 'email' \
                 THEN regexp_replace(r.error, '<?[^[:space:]<>@]+@[^[:space:]<>]+>?', '<адрес>', 'g') \
                 ELSE r.error END AS reason \
             FROM broadcast_recipients r JOIN broadcasts b ON b.id = r.broadcast_id \
             WHERE b.rule_id = $1 AND r.state = 'failed' AND r.error IS NOT NULL) f \
         GROUP BY channel, reason ORDER BY count(*) DESC, channel, reason LIMIT $2",
    )
    .bind(id)
    .bind(PROBLEMS)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(channel, error, count)| {
            Lane::parse(&channel).map(|channel| Problem {
                channel,
                error,
                count,
            })
        })
        .collect())
}

pub async fn sending_notices(db: &mut impl Db, id: i64) -> StoreResult<Vec<String>> {
    let rows: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT notice, mail_notice FROM broadcasts WHERE rule_id = $1 AND status = 'sending' ORDER BY id",
    )
    .bind(id)
    .fetch_all(db.conn())
    .await?;
    let mut out: Vec<String> = Vec::new();
    for notice in rows
        .into_iter()
        .flat_map(|(bot, mail)| [bot, mail])
        .flatten()
    {
        if !out.contains(&notice) {
            out.push(notice);
        }
    }
    Ok(out)
}

pub async fn set_checked(
    db: &mut impl Db,
    id: i64,
    at: DateTime<Utc>,
    notice: Option<&str>,
) -> StoreResult<()> {
    sqlx::query("UPDATE auto_broadcasts SET checked_at = $2, notice = $3 WHERE id = $1")
        .bind(id)
        .bind(at)
        .bind(notice)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub struct Bounds {
    pub now: DateTime<Utc>,
    pub from: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl Bounds {
    pub fn of(trigger: Trigger, span: Duration, now: DateTime<Utc>, window: Duration) -> Self {
        let until = match trigger {
            Trigger::Expiring | Trigger::TrialExpiring => now + span,
            _ => now - span,
        };
        Bounds {
            now,
            from: until - window,
            until,
        }
    }
}

fn hour_key(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', 'YYYYMMDDHH24')")
}

const BOUGHT: &str = "(EXISTS (SELECT 1 FROM orders o WHERE o.client_id = {c} AND o.paid_at IS NOT NULL \
                      AND o.kind IN ('new', 'renewal', 'change')) \
                      OR EXISTS (SELECT 1 FROM subscriptions p WHERE p.client_id = {c} AND p.kind = 'paid'))";

fn bought(client: &str) -> String {
    BOUGHT.replace("{c}", client)
}

const OTHER_LIVE: &str = "EXISTS (SELECT 1 FROM subscriptions y WHERE y.client_id = s.client_id AND y.id <> s.id \
                          AND y.ended_at IS NULL AND y.remna_missing_at IS NULL AND y.expire_at > $1 \
                          AND (y.status IS NULL OR y.status IN ('active', 'limited')))";

const ENDED_BY_TIME: &str = "(s.ended_at IS NULL OR s.ended_at >= s.expire_at) \
                             AND (s.remna_missing_at IS NULL OR s.remna_missing_at >= s.expire_at) \
                             AND (s.status IS NULL OR s.status <> 'disabled')";

const LIVE: &str = "s.ended_at IS NULL AND s.remna_missing_at IS NULL \
                    AND (s.status IS NULL OR s.status IN ('active', 'limited'))";

const OVER: &str = "s.traffic_limit_bytes > 0 AND s.traffic_used_bytes IS NOT NULL \
                    AND s.traffic_used_bytes::numeric * 100 >= s.traffic_limit_bytes::numeric * $7";

const SUB: &str = "s.tariff_id AS tariff_id, s.autorenew_enabled AS autorenew";

const NO_SUB: &str = "NULL::bigint AS tariff_id, NULL::boolean AS autorenew";

fn cases_sql(trigger: Trigger) -> String {
    let delay = "make_interval(secs => $4)";
    let key = hour_key("s.expire_at");
    match trigger {
        Trigger::Expiring => format!(
            "SELECT s.client_id, 'expiring:' || s.id || ':' || {key} AS case_key, \
             s.expire_at - {delay} AS due_at, {SUB} \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.expire_at > $1 AND s.kind <> 'trial' AND {LIVE} \
             AND ($5 OR NOT s.autorenew_enabled)"
        ),
        Trigger::TrialExpiring => format!(
            "SELECT s.client_id, 'trial_expiring:' || s.id || ':' || {key} AS case_key, \
             s.expire_at - {delay} AS due_at, {SUB} \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.expire_at > $1 AND s.kind = 'trial' AND {LIVE} \
             AND NOT {bought}",
            bought = bought("s.client_id")
        ),
        Trigger::Expired => format!(
            "SELECT s.client_id, 'expired:' || s.id || ':' || {key} AS case_key, \
             s.expire_at + {delay} AS due_at, {SUB} \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.kind <> 'trial' AND {ENDED_BY_TIME} \
             AND NOT {OTHER_LIVE}"
        ),
        Trigger::TrialEnded => format!(
            "SELECT s.client_id, 'trial:' || s.id || ':' || {key} AS case_key, \
             s.expire_at + {delay} AS due_at, {SUB} \
             FROM subscriptions s \
             WHERE s.expire_at > $2 AND s.expire_at <= $3 AND s.kind = 'trial' AND {ENDED_BY_TIME} \
             AND NOT {OTHER_LIVE} AND NOT {bought}",
            bought = bought("s.client_id")
        ),
        Trigger::NoPurchase => format!(
            "SELECT c.id AS client_id, 'registered:' || c.id AS case_key, \
             c.created_at + {delay} AS due_at, {NO_SUB} \
             FROM clients c \
             WHERE c.created_at > $2 AND c.created_at <= $3 AND c.deleted_at IS NULL \
             AND c.registration_source NOT IN ('import', 'migration') AND NOT {bought}",
            bought = bought("c.id")
        ),
        Trigger::NoSubscription => format!(
            "SELECT c.id AS client_id, 'nosub:' || c.id AS case_key, \
             c.created_at + {delay} AS due_at, {NO_SUB} \
             FROM clients c \
             WHERE c.created_at > $2 AND c.created_at <= $3 AND c.deleted_at IS NULL \
             AND c.registration_source NOT IN ('import', 'migration') \
             AND NOT EXISTS (SELECT 1 FROM subscriptions p WHERE p.client_id = c.id) AND NOT {bought}",
            bought = bought("c.id")
        ),
        Trigger::NotConnected => format!(
            "SELECT s.client_id, 'connect:' || s.id AS case_key, \
             p.paid_at + {delay} AS due_at, {SUB} \
             FROM subscriptions s \
             JOIN LATERAL (SELECT min(o.paid_at) AS paid_at FROM orders o WHERE o.subscription_id = s.id \
                 AND o.paid_at IS NOT NULL AND o.kind IN ('new', 'renewal', 'change')) p ON true \
             WHERE p.paid_at > $2 AND p.paid_at <= $3 AND s.first_connected_at IS NULL \
             AND s.expire_at > $1 AND {LIVE}"
        ),
        Trigger::UnpaidOrder => format!(
            "SELECT o.client_id, 'unpaid:' || o.id AS case_key, o.created_at + {delay} AS due_at, \
             CASE WHEN jsonb_typeof(o.intent -> 'tariff_id') = 'number' \
                 THEN (o.intent ->> 'tariff_id')::bigint END AS tariff_id, NULL::boolean AS autorenew \
             FROM orders o \
             WHERE o.created_at > $2 AND o.created_at <= $3 AND o.paid_at IS NULL \
             AND o.kind IN ('new', 'renewal', 'change') \
             AND o.channel IS DISTINCT FROM 'autorenew' AND o.channel IS DISTINCT FROM 'admin' \
             AND NOT EXISTS (SELECT 1 FROM orders n WHERE n.client_id = o.client_id AND n.id > o.id \
                 AND n.kind IN ('new', 'renewal', 'change')) \
             AND NOT EXISTS (SELECT 1 FROM orders p WHERE p.client_id = o.client_id \
                 AND p.paid_at >= o.created_at)"
        ),
        Trigger::FirstPurchase => format!(
            "SELECT o.client_id, 'first:' || o.client_id AS case_key, o.paid_at + {delay} AS due_at, \
             s.tariff_id AS tariff_id, s.autorenew_enabled AS autorenew \
             FROM orders o \
             JOIN clients fc ON fc.id = o.client_id AND fc.registration_source NOT IN ('import', 'migration') \
             LEFT JOIN subscriptions s ON s.id = o.subscription_id \
             WHERE o.paid_at > $2 AND o.paid_at <= $3 AND o.kind IN ('new', 'renewal', 'change') \
             AND NOT EXISTS (SELECT 1 FROM orders e WHERE e.client_id = o.client_id \
                 AND e.kind IN ('new', 'renewal', 'change') AND e.paid_at IS NOT NULL \
                 AND (e.paid_at, e.id) < (o.paid_at, o.id)) \
             AND NOT EXISTS (SELECT 1 FROM subscriptions p WHERE p.client_id = o.client_id \
                 AND p.kind = 'paid' AND p.created_at < o.created_at)"
        ),
        Trigger::Idle => format!(
            "SELECT s.client_id, 'idle:' || s.id || ':' || {idle} AS case_key, \
             s.online_at + {delay} AS due_at, {SUB} \
             FROM subscriptions s \
             WHERE s.online_at > $2 AND s.online_at <= $3 AND s.ended_at IS NULL \
             AND s.remna_missing_at IS NULL AND (s.status IS NULL OR s.status = 'active') \
             AND s.expire_at > $1 AND s.synced_at >= s.online_at + {delay}",
            idle = hour_key("s.online_at")
        ),
        Trigger::DevicesFull => format!(
            "SELECT s.client_id, 'devices:' || s.id || ':' || s.hwid_limit AS case_key, \
             $1::timestamptz AS due_at, {SUB} \
             FROM subscriptions s \
             JOIN tariffs dt ON dt.id = s.tariff_id \
             WHERE s.ended_at IS NULL AND s.remna_missing_at IS NULL \
             AND (s.status IS NULL OR s.status = 'active') AND s.expire_at > $1 AND s.kind <> 'trial' \
             AND s.hwid_limit > 0 AND s.devices_count >= s.hwid_limit \
             AND dt.extra_devices_enabled AND s.extra_devices < dt.extra_devices_max"
        ),
        Trigger::TrafficLow => format!(
            "SELECT s.client_id, 'traffic:' || s.id || ':' || (COALESCE(t.seq, 0) + 1) AS case_key, \
             $1::timestamptz AS due_at, {SUB} \
             FROM subscriptions s \
             LEFT JOIN auto_broadcast_traffic t ON t.rule_id = $6 AND t.subscription_id = s.id \
             WHERE s.ended_at IS NULL AND s.traffic_limit_bytes > 0 AND s.expire_at > $1 AND {LIVE} \
             AND {OVER} AND (t.subscription_id IS NULL OR t.low)"
        ),
    }
}

fn found_sql(trigger: Trigger, tail: &str) -> String {
    format!(
        "WITH found AS ({cases}) \
         SELECT f.client_id, f.case_key, f.due_at, tg.value AS telegram, \
             c.bot_unreachable AS stopped, em.value AS email, \
             EXISTS (SELECT 1 FROM broadcast_unsubscribes u WHERE u.email = em.value) AS unsubscribed, \
             CASE WHEN $8 AND c.locale = 'en' THEN 'en' ELSE 'ru' END AS locale \
         FROM found f \
         JOIN clients c ON c.id = f.client_id AND c.deleted_at IS NULL AND c.status = 'active' \
         LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id \
             AND i.kind = 'telegram' AND i.value ~ '^[0-9]{{1,15}}$' ORDER BY i.id LIMIT 1) tg ON true \
         LEFT JOIN LATERAL (SELECT lower(i.value) AS value FROM client_identities i WHERE i.client_id = c.id \
             AND i.kind = 'email' AND i.value LIKE '%_@_%' AND (i.verified OR NOT i.meta ? 'no_code') \
             ORDER BY i.id LIMIT 1) em ON true \
         LEFT JOIN LATERAL (SELECT true AS hit FROM auto_broadcast_cases k WHERE k.rule_id = $6 \
             AND k.case_key = f.case_key LIMIT 1) seen ON true \
         WHERE seen.hit IS NULL{tail}",
        cases = cases_sql(trigger)
    )
}

fn audience_tail(
    audience: &ClientConditions,
    args: &mut SqlArgs,
    now: DateTime<Utc>,
    currency: Currency,
) -> StoreResult<String> {
    let mut rest = audience.clone();
    let tariffs = std::mem::take(&mut rest.tariff);
    let categories = std::mem::take(&mut rest.category);
    let autorenew = rest.autorenew.take();
    let mut parts = rest.conditions(args, now, currency)?;
    if !tariffs.is_empty() {
        let list = args.push(SqlArg::Ints(tariffs.iter().map(|t| t.0).collect()));
        parts.push(format!("f.tariff_id = ANY({list})"));
    }
    if !categories.is_empty() {
        let list = args.push(SqlArg::Ints(categories.iter().map(|c| c.0).collect()));
        parts.push(format!(
            "f.tariff_id IN (SELECT ft.id FROM tariffs ft WHERE ft.category_id = ANY({list}))"
        ));
    }
    match autorenew {
        Some(true) => parts.push("f.autorenew IS TRUE".into()),
        Some(false) => parts.push("f.autorenew IS FALSE".into()),
        None => {}
    }
    Ok(parts.iter().map(|p| format!(" AND {p}")).collect())
}

fn probe_args(probe: &Probe, bounds: &Bounds, limit: Option<i64>) -> SqlArgs {
    let mut args = SqlArgs::default();
    args.push(SqlArg::Time(bounds.now));
    args.push(SqlArg::Time(bounds.from));
    args.push(SqlArg::Time(bounds.until));
    args.push(SqlArg::Int(probe.delay.span().num_seconds()));
    args.push(SqlArg::Flag(probe.with_autorenew));
    args.push(SqlArg::Int(probe.rule.unwrap_or(0)));
    args.push(SqlArg::Int(i64::from(probe.percent())));
    args.push(SqlArg::Flag(probe.english));
    if let Some(limit) = limit {
        args.push(SqlArg::Int(limit));
    }
    args
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Found {
    pub client_id: i64,
    pub case_key: String,
    pub due_at: DateTime<Utc>,
    pub telegram: Option<String>,
    pub stopped: bool,
    pub email: Option<String>,
    pub unsubscribed: bool,
    pub locale: String,
}

impl Found {
    pub fn chat(&self) -> Option<i64> {
        if self.stopped {
            return None;
        }
        self.telegram
            .as_deref()
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
    }

    pub fn mailbox(&self) -> Option<&str> {
        if self.unsubscribed {
            return None;
        }
        self.email.as_deref()
    }

    pub fn locale(&self) -> Locale {
        Locale::parse(&self.locale).unwrap_or_default()
    }
}

pub struct Probe {
    pub trigger: Trigger,
    pub delay: Delay,
    pub percent: Option<u8>,
    pub with_autorenew: bool,
    pub rule: Option<i64>,
    pub english: bool,
    pub audience: Option<ClientConditions>,
    pub currency: Currency,
}

impl Probe {
    fn seconds(&self) -> f64 {
        self.delay.span().num_seconds() as f64
    }

    fn percent(&self) -> i32 {
        i32::from(self.percent.unwrap_or(super::auto::PERCENT_DEFAULT))
    }
}

pub async fn found(
    db: &mut impl Db,
    probe: &Probe,
    bounds: &Bounds,
    limit: i64,
) -> StoreResult<Vec<Found>> {
    if let Some(audience) = &probe.audience {
        let mut args = probe_args(probe, bounds, Some(limit));
        let tail = audience_tail(audience, &mut args, bounds.now, probe.currency)?;
        let sql = format!(
            "SELECT * FROM ({}) q ORDER BY q.due_at, q.case_key LIMIT $9",
            found_sql(probe.trigger, &tail)
        );
        return Ok(args
            .bind_as(sqlx::query_as::<_, Found>(&sql).persistent(false))
            .fetch_all(db.conn())
            .await?);
    }
    Ok(sqlx::query_as::<_, Found>(&format!(
        "SELECT * FROM ({}) q ORDER BY q.due_at, q.case_key LIMIT $9",
        found_sql(probe.trigger, "")
    ))
    .bind(bounds.now)
    .bind(bounds.from)
    .bind(bounds.until)
    .bind(probe.seconds())
    .bind(probe.with_autorenew)
    .bind(probe.rule)
    .bind(probe.percent())
    .bind(probe.english)
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counted {
    pub cases: i64,
    pub recipients: i64,
    pub stopped: i64,
    pub emails: i64,
    pub unsubscribed: i64,
}

const COUNTS: &str = "SELECT count(*), \
     count(*) FILTER (WHERE q.telegram IS NOT NULL AND NOT q.stopped), \
     count(*) FILTER (WHERE q.telegram IS NOT NULL AND q.stopped), \
     count(*) FILTER (WHERE q.email IS NOT NULL AND NOT q.unsubscribed), \
     count(*) FILTER (WHERE q.email IS NOT NULL AND q.unsubscribed)";

pub async fn count(db: &mut impl Db, probe: &Probe, bounds: &Bounds) -> StoreResult<Counted> {
    let (cases, recipients, stopped, emails, unsubscribed): (i64, i64, i64, i64, i64) =
        if let Some(audience) = &probe.audience {
            let mut args = probe_args(probe, bounds, None);
            let tail = audience_tail(audience, &mut args, bounds.now, probe.currency)?;
            let sql = format!("{COUNTS} FROM ({}) q", found_sql(probe.trigger, &tail));
            args.bind_as(sqlx::query_as(&sql).persistent(false))
                .fetch_one(db.conn())
                .await?
        } else {
            sqlx::query_as(&format!(
                "{COUNTS} FROM ({}) q",
                found_sql(probe.trigger, "")
            ))
            .bind(bounds.now)
            .bind(bounds.from)
            .bind(bounds.until)
            .bind(probe.seconds())
            .bind(probe.with_autorenew)
            .bind(probe.rule)
            .bind(probe.percent())
            .bind(probe.english)
            .fetch_one(db.conn())
            .await?
        };
    Ok(Counted {
        cases,
        recipients,
        stopped,
        emails,
        unsubscribed,
    })
}

pub async fn rearm_traffic(db: &mut impl Db, rule: i64, percent: u8) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE auto_broadcast_traffic t SET low = true FROM subscriptions s \
         WHERE t.rule_id = $1 AND t.subscription_id = s.id AND NOT t.low AND s.traffic_used_bytes IS NOT NULL \
         AND (COALESCE(s.traffic_limit_bytes, 0) <= 0 \
             OR s.traffic_used_bytes::numeric * 100 < s.traffic_limit_bytes::numeric * $2)",
    )
    .bind(rule)
    .bind(i32::from(percent))
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn mark_traffic(db: &mut impl Db, rule: i64, fired: &[(i64, i32)]) -> StoreResult<()> {
    let subscriptions: Vec<i64> = fired.iter().map(|(s, _)| *s).collect();
    let seqs: Vec<i32> = fired.iter().map(|(_, q)| *q).collect();
    sqlx::query(
        "INSERT INTO auto_broadcast_traffic (rule_id, subscription_id, seq, low) \
         SELECT $1, t.s, t.q, false FROM unnest($2::bigint[], $3::int[]) AS t(s, q) \
         ON CONFLICT (rule_id, subscription_id) DO UPDATE SET seq = EXCLUDED.seq, low = false",
    )
    .bind(rule)
    .bind(&subscriptions)
    .bind(&seqs)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn batch(
    db: &mut impl Db,
    rule: i64,
    day: NaiveDate,
    version: i32,
    locale: Locale,
) -> StoreResult<i64> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM broadcasts WHERE rule_id = $1 AND rule_day = $2 AND rule_version = $3 \
         AND rule_locale = $4 FOR UPDATE",
    )
    .bind(rule)
    .bind(day)
    .bind(version)
    .bind(locale.as_str())
    .fetch_optional(db.conn())
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    Ok(sqlx::query_scalar(
        "INSERT INTO broadcasts (text, image_id, buttons, audience, channel, subject, status, rule_id, rule_day, \
         rule_version, rule_locale) \
         SELECT CASE WHEN $4 = 'en' AND a.en IS NOT NULL THEN a.en ->> 'text' ELSE a.text END, a.image_id, \
             CASE WHEN $4 = 'en' AND a.en IS NOT NULL THEN COALESCE(a.en -> 'buttons', '[]'::jsonb) ELSE a.buttons END, \
             '{\"kind\": \"all\"}'::jsonb, a.channel, \
             CASE WHEN $4 = 'en' AND a.en IS NOT NULL THEN COALESCE(NULLIF(a.en ->> 'subject', ''), a.subject) \
                 ELSE a.subject END, \
             'done', a.id, $2, $3, $4 \
         FROM auto_broadcasts a WHERE a.id = $1 RETURNING id",
    )
    .bind(rule)
    .bind(day)
    .bind(version)
    .bind(locale.as_str())
    .fetch_one(db.conn())
    .await?)
}

pub struct NewCase<'a> {
    pub key: &'a str,
    pub client_id: ClientId,
    pub due_at: DateTime<Utc>,
    pub batch: Option<i64>,
}

pub async fn record_cases(
    db: &mut impl Db,
    rule: i64,
    cases: &[NewCase<'_>],
) -> StoreResult<Vec<String>> {
    let keys: Vec<&str> = cases.iter().map(|c| c.key).collect();
    let clients: Vec<i64> = cases.iter().map(|c| c.client_id.0).collect();
    let due: Vec<DateTime<Utc>> = cases.iter().map(|c| c.due_at).collect();
    let batches: Vec<Option<i64>> = cases.iter().map(|c| c.batch).collect();
    Ok(sqlx::query_scalar(
        "INSERT INTO auto_broadcast_cases (rule_id, case_key, client_id, due_at, broadcast_id, queued) \
         SELECT $1, t.k, t.c, t.d, t.b, t.b IS NOT NULL \
         FROM unnest($2::text[], $3::bigint[], $4::timestamptz[], $5::bigint[]) AS t(k, c, d, b) \
         ON CONFLICT DO NOTHING RETURNING case_key",
    )
    .bind(rule)
    .bind(&keys)
    .bind(&clients)
    .bind(&due)
    .bind(&batches)
    .fetch_all(db.conn())
    .await?)
}

pub async fn queue_bot(db: &mut impl Db, batch: i64, rows: &[(ClientId, i64)]) -> StoreResult<u64> {
    if rows.is_empty() {
        return Ok(0);
    }
    let clients: Vec<i64> = rows.iter().map(|(c, _)| c.0).collect();
    let chats: Vec<i64> = rows.iter().map(|(_, chat)| *chat).collect();
    Ok(sqlx::query(
        "INSERT INTO broadcast_recipients (broadcast_id, channel, client_id, chat_id) \
         SELECT $1, 'bot', t.c, t.chat FROM unnest($2::bigint[], $3::bigint[]) AS t(c, chat) \
         ON CONFLICT DO NOTHING",
    )
    .bind(batch)
    .bind(&clients)
    .bind(&chats)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn queue_mail(
    db: &mut impl Db,
    batch: i64,
    rows: &[(ClientId, String)],
) -> StoreResult<u64> {
    if rows.is_empty() {
        return Ok(0);
    }
    let clients: Vec<i64> = rows.iter().map(|(c, _)| c.0).collect();
    let emails: Vec<&str> = rows.iter().map(|(_, e)| e.as_str()).collect();
    Ok(sqlx::query(
        "INSERT INTO broadcast_recipients (broadcast_id, channel, client_id, email, unsubscribe) \
         SELECT $1, 'email', t.c, t.e, gen_random_uuid() FROM unnest($2::bigint[], $3::text[]) AS t(c, e) \
         ON CONFLICT DO NOTHING",
    )
    .bind(batch)
    .bind(&clients)
    .bind(&emails)
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn open_batch(db: &mut impl Db, batch: i64, now: DateTime<Utc>) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'sending', started_at = COALESCE(started_at, $2), finished_at = NULL, \
         total = LEAST((SELECT count(*) FROM broadcast_recipients r WHERE r.broadcast_id = $1), 2147483647)::int \
         WHERE id = $1",
    )
    .bind(batch)
    .bind(now)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn contest_batch(
    db: &mut impl Db,
    message_id: i64,
    locale: Locale,
    text: &str,
    buttons: &serde_json::Value,
) -> StoreResult<i64> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM broadcasts WHERE contest_message_id = $1 AND contest_locale = $2 FOR UPDATE",
    )
    .bind(message_id)
    .bind(locale.as_str())
    .fetch_optional(db.conn())
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    Ok(sqlx::query_scalar(
        "INSERT INTO broadcasts (text, buttons, audience, channel, status, contest_message_id, contest_locale) \
         VALUES ($1, $2, '{\"kind\": \"all\"}'::jsonb, 'bot', 'done', $3, $4) RETURNING id",
    )
    .bind(text)
    .bind(buttons)
    .bind(message_id)
    .bind(locale.as_str())
    .fetch_one(db.conn())
    .await?)
}

pub async fn contest_progress(
    db: &mut impl Db,
    message_ids: &[i64],
) -> StoreResult<HashMap<i64, Progress>> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT b.contest_message_id, r.state, count(*) FROM broadcasts b \
         JOIN broadcast_recipients r ON r.broadcast_id = b.id \
         WHERE b.contest_message_id = ANY($1) GROUP BY b.contest_message_id, r.state",
    )
    .bind(message_ids)
    .fetch_all(db.conn())
    .await?;
    let mut out: HashMap<i64, Progress> = HashMap::new();
    for (id, state, count) in rows {
        let progress = out.entry(id).or_default();
        progress.total += count;
        match state.as_str() {
            "sent" => progress.sent += count,
            "failed" => progress.failed += count,
            "blocked" => progress.blocked += count,
            "skipped" => progress.skipped += count,
            _ => progress.pending += count,
        }
    }
    Ok(out)
}

pub async fn stop_contest(db: &mut impl Db, contest_id: i64, reason: &str) -> StoreResult<u64> {
    Ok(sqlx::query(
        "UPDATE broadcast_recipients r SET state = 'skipped', done_at = now(), error = $2 \
         FROM broadcasts b JOIN contest_messages m ON m.id = b.contest_message_id \
         WHERE b.id = r.broadcast_id AND m.contest_id = $1 AND r.state = 'pending'",
    )
    .bind(contest_id)
    .bind(reason)
    .execute(db.conn())
    .await?
    .rows_affected())
}
