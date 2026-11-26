use super::{Audience, AudienceNames, ButtonSpec, Channel, Cleaned, Lane, Status};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::client::Locale;
use sn_domain::ids::{AdminId, ClientId, TariffId};
use sn_domain::image::ImageInfo;
use sn_domain::money::Currency;
use sn_domain::paging::{Page, PageRequest};
use sn_store::{ClientConditions, ClientSegment, Db, SqlArg, SqlArgs, StoreError, StoreResult};
use std::collections::HashMap;
use uuid::Uuid;

const PREVIEW_CHARS: usize = 120;
const PROBLEMS: i64 = 6;
const FAILED_EMAILS: i64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageMeta {
    pub id: i64,
    pub content_type: String,
    pub width: i32,
    pub height: i32,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ImageData {
    pub id: i64,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub file_bot: Option<i64>,
    pub file_id: Option<String>,
    pub public_key: Uuid,
}

impl ImageData {
    pub fn file_for(&self, bot: Option<i64>) -> Option<&str> {
        match (bot, self.file_bot) {
            (Some(bot), Some(file_bot)) if bot == file_bot => self.file_id.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub total: i64,
    pub sent: i64,
    pub failed: i64,
    pub blocked: i64,
    pub skipped: i64,
    pub pending: i64,
}

impl Progress {
    fn add(&mut self, other: &Progress) {
        self.total += other.total;
        self.sent += other.sent;
        self.failed += other.failed;
        self.blocked += other.blocked;
        self.skipped += other.skipped;
        self.pending += other.pending;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ChannelProgress {
    pub channel: Lane,
    pub progress: Progress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub channel: Lane,
    pub error: String,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailedEmail {
    pub email: String,
    pub error: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Reach {
    pub recipients: i64,
    pub stopped: i64,
    pub emails: i64,
    pub unsubscribed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Broadcast {
    pub id: i64,
    pub status: Status,
    pub channel: Channel,
    pub subject: Option<String>,
    pub text: String,
    pub image: Option<ImageMeta>,
    pub buttons: Vec<ButtonSpec>,
    pub audience: Audience,
    pub audience_label: String,
    pub send_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub notice: Option<String>,
    pub mail_notice: Option<String>,
    pub progress: Progress,
    pub channels: Vec<ChannelProgress>,
    pub problems: Vec<Problem>,
    pub failed_emails: Vec<FailedEmail>,
    pub queued_behind: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListItem {
    pub id: i64,
    pub status: Status,
    pub channel: Channel,
    pub preview: String,
    pub has_image: bool,
    pub buttons: usize,
    pub audience: Audience,
    pub audience_label: String,
    pub send_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub notice: Option<String>,
    pub mail_notice: Option<String>,
    pub progress: Progress,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    channel: String,
    subject: Option<String>,
    text: String,
    image_id: Option<i64>,
    buttons: serde_json::Value,
    audience: serde_json::Value,
    status: String,
    send_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    notice: Option<String>,
    mail_notice: Option<String>,
    tariff_name: Option<String>,
}

const COLUMNS: &str = "b.id, b.channel, b.subject, b.text, b.image_id, b.buttons, b.audience, b.status, \
                       b.send_at, b.started_at, b.finished_at, b.created_at, b.updated_at, b.notice, b.mail_notice, \
                       (SELECT t.name FROM tariffs t WHERE t.id = (b.audience ->> 'tariff_id')::bigint) AS tariff_name";

struct Parsed {
    status: Status,
    channel: Channel,
    buttons: Vec<ButtonSpec>,
    audience: Audience,
    audience_label: String,
}

async fn names_for(db: &mut impl Db, rows: &[Row]) -> StoreResult<Option<AudienceNames>> {
    if !rows
        .iter()
        .any(|row| row.audience.get("kind").and_then(|k| k.as_str()) == Some("filter"))
    {
        return Ok(None);
    }
    let options = sn_store::filter_options(db.conn()).await?;
    Ok(Some(AudienceNames {
        tariffs: options.tariff_names(),
        categories: options.category_names(),
        currency: crate::catalog::installation_currency(db).await?,
    }))
}

fn parse(row: &Row, names: Option<&AudienceNames>) -> StoreResult<Parsed> {
    let status = Status::parse(&row.status)
        .ok_or_else(|| StoreError::Decode(format!("состояние рассылки: {}", row.status)))?;
    let channel = Channel::parse(&row.channel)
        .ok_or_else(|| StoreError::Decode(format!("канал рассылки: {}", row.channel)))?;
    let buttons: Vec<ButtonSpec> = serde_json::from_value(row.buttons.clone())
        .map_err(|e| StoreError::Decode(format!("кнопки рассылки: {e}")))?;
    let audience: Audience = serde_json::from_value(row.audience.clone())
        .map_err(|e| StoreError::Decode(format!("аудитория рассылки: {e}")))?;
    Ok(Parsed {
        status,
        channel,
        audience_label: audience.describe(row.tariff_name.as_deref(), names),
        buttons,
        audience,
    })
}

fn json<T: Serialize>(value: &T) -> StoreResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| StoreError::Invalid(e.to_string()))
}

pub fn preview(html: &str) -> String {
    let mut out = String::with_capacity(html.len().min(PREVIEW_CHARS * 4));
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    let line = decoded
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= PREVIEW_CHARS {
        line
    } else {
        let mut cut: String = line.chars().take(PREVIEW_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

pub async fn insert(
    db: &mut impl Db,
    content: &Cleaned,
    admin: Option<AdminId>,
) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO broadcasts (text, image_id, buttons, audience, channel, subject, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(json(&content.audience)?)
    .bind(content.channel.as_str())
    .bind(content.subject.as_deref())
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await?)
}

pub async fn update(db: &mut impl Db, id: i64, content: &Cleaned) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET text = $2, image_id = $3, buttons = $4, audience = $5, channel = $6, \
         subject = $7 WHERE id = $1",
    )
    .bind(id)
    .bind(&content.text)
    .bind(content.image_id)
    .bind(json(&content.buttons)?)
    .bind(json(&content.audience)?)
    .bind(content.channel.as_str())
    .bind(content.subject.as_deref())
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn delete(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query("DELETE FROM broadcasts WHERE id = $1")
        .bind(id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn lock(db: &mut impl Db, id: i64) -> StoreResult<Status> {
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM broadcasts WHERE id = $1 AND rule_id IS NULL AND contest_message_id IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    let status = status.ok_or(StoreError::NotFound("рассылка"))?;
    Status::parse(&status)
        .ok_or_else(|| StoreError::Decode(format!("состояние рассылки: {status}")))
}

type Tally = (Progress, Vec<ChannelProgress>);

type LaneRow = (i64, String, i64, i64, i64, i64, i64, i64);

async fn progress_of(db: &mut impl Db, ids: &[i64]) -> StoreResult<HashMap<i64, Tally>> {
    let rows: Vec<LaneRow> = sqlx::query_as(
        "SELECT broadcast_id, channel, count(*), \
         count(*) FILTER (WHERE state = 'sent'), count(*) FILTER (WHERE state = 'failed'), \
         count(*) FILTER (WHERE state = 'blocked'), count(*) FILTER (WHERE state = 'skipped'), \
         count(*) FILTER (WHERE state IN ('pending', 'sending')) \
         FROM broadcast_recipients WHERE broadcast_id = ANY($1) GROUP BY broadcast_id, channel \
         ORDER BY broadcast_id, channel",
    )
    .bind(ids)
    .fetch_all(db.conn())
    .await?;
    let mut out: HashMap<i64, Tally> = HashMap::new();
    for (id, channel, total, sent, failed, blocked, skipped, pending) in rows {
        let progress = Progress {
            total,
            sent,
            failed,
            blocked,
            skipped,
            pending,
        };
        let entry = out.entry(id).or_default();
        entry.0.add(&progress);
        if let Some(channel) = Lane::parse(&channel) {
            entry.1.push(ChannelProgress { channel, progress });
        }
    }
    Ok(out)
}

pub async fn image_meta(db: &mut impl Db, id: i64) -> StoreResult<Option<ImageMeta>> {
    let row: Option<(i64, String, i32, i32, i64)> = sqlx::query_as(
        "SELECT id, content_type, width, height, octet_length(bytes)::bigint FROM broadcast_images WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db.conn())
    .await?;
    Ok(
        row.map(|(id, content_type, width, height, size)| ImageMeta {
            id,
            content_type,
            width,
            height,
            size,
        }),
    )
}

pub async fn view(db: &mut impl Db, id: i64) -> StoreResult<Broadcast> {
    let row: Row = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM broadcasts b WHERE b.id = $1 AND b.rule_id IS NULL AND b.contest_message_id IS NULL"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("рассылка"))?;
    let names = names_for(db, std::slice::from_ref(&row)).await?;
    let parsed = parse(&row, names.as_ref())?;
    let image = match row.image_id {
        Some(image) => image_meta(db, image).await?,
        None => None,
    };
    let (progress, channels) = progress_of(db, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default();
    let problems: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT channel, reason, count(*) FROM ( \
             SELECT channel, CASE WHEN channel = 'email' \
                 THEN regexp_replace(error, '<?[^[:space:]<>@]+@[^[:space:]<>]+>?', '<адрес>', 'g') \
                 ELSE error END AS reason \
             FROM broadcast_recipients \
             WHERE broadcast_id = $1 AND state = 'failed' AND error IS NOT NULL) f \
         GROUP BY channel, reason ORDER BY count(*) DESC, channel, reason LIMIT $2",
    )
    .bind(id)
    .bind(PROBLEMS)
    .fetch_all(db.conn())
    .await?;
    let failed_emails: Vec<(String, String)> = sqlx::query_as(
        "SELECT email, coalesce(error, '') FROM broadcast_recipients \
         WHERE broadcast_id = $1 AND channel = 'email' AND state = 'failed' AND email IS NOT NULL \
         ORDER BY email LIMIT $2",
    )
    .bind(id)
    .bind(FAILED_EMAILS)
    .fetch_all(db.conn())
    .await?;
    let queued_behind = if parsed.status == Status::Sending {
        sqlx::query_scalar(
            "SELECT b.id FROM broadcasts b WHERE b.status = 'sending' AND b.rule_id IS NULL AND b.contest_message_id IS NULL \
             AND (b.started_at, b.id) < ($1, $2) ORDER BY b.started_at, b.id LIMIT 1",
        )
        .bind(row.started_at)
        .bind(id)
        .fetch_optional(db.conn())
        .await?
    } else {
        None
    };
    Ok(Broadcast {
        id: row.id,
        status: parsed.status,
        channel: parsed.channel,
        subject: row.subject,
        text: row.text,
        image,
        buttons: parsed.buttons,
        audience: parsed.audience,
        audience_label: parsed.audience_label,
        send_at: row.send_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        notice: row.notice,
        mail_notice: row.mail_notice,
        progress,
        channels,
        problems: problems
            .into_iter()
            .filter_map(|(channel, error, count)| {
                Lane::parse(&channel).map(|channel| Problem {
                    channel,
                    error,
                    count,
                })
            })
            .collect(),
        failed_emails: failed_emails
            .into_iter()
            .map(|(email, error)| FailedEmail { email, error })
            .collect(),
        queued_behind,
    })
}

pub async fn list(db: &mut impl Db, page: PageRequest) -> StoreResult<Page<ListItem>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM broadcasts b WHERE b.rule_id IS NULL AND b.contest_message_id IS NULL AND ($1::bigint IS NULL OR b.id < $1) \
         ORDER BY b.id DESC LIMIT $2"
    ))
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut progress = progress_of(db, &ids).await?;
    let names = names_for(db, &rows).await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let parsed = parse(&row, names.as_ref())?;
            Ok(ListItem {
                id: row.id,
                status: parsed.status,
                channel: parsed.channel,
                preview: preview(&row.text),
                has_image: row.image_id.is_some(),
                buttons: parsed.buttons.len(),
                audience: parsed.audience,
                audience_label: parsed.audience_label,
                send_at: row.send_at,
                started_at: row.started_at,
                finished_at: row.finished_at,
                created_at: row.created_at,
                notice: row.notice,
                mail_notice: row.mail_notice,
                progress: progress.remove(&row.id).map(|p| p.0).unwrap_or_default(),
            })
        })
        .collect::<StoreResult<Vec<ListItem>>>()?;
    Ok(Page::from_overfetch(items, page, |item| item.id))
}

fn status_of(alias: &str) -> String {
    format!(
        "COALESCE({alias}.status, CASE WHEN {alias}.expire_at IS NULL THEN NULL \
         WHEN {alias}.expire_at <= $1 THEN 'expired' ELSE 'active' END)"
    )
}

fn rank_of(alias: &str) -> String {
    format!(
        "CASE {} WHEN 'active' THEN 0 WHEN 'limited' THEN 1 WHEN 'disabled' THEN 2 \
         WHEN 'expired' THEN 3 ELSE 4 END",
        status_of(alias)
    )
}

fn condition(audience: &Audience) -> String {
    let s = status_of("s");
    match audience {
        Audience::All => "TRUE".into(),
        Audience::Active => format!("{s} = 'active' AND s.kind <> 'trial'"),
        Audience::Expiring { .. } => {
            format!("{s} = 'active' AND s.expire_at > $1 AND s.expire_at <= $2")
        }
        Audience::Expired => format!(
            "EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id) \
             AND NOT EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id \
             AND h.ended_at IS NULL AND h.remna_missing_at IS NULL AND {h} < 3) \
             AND NOT EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id \
             AND h.ended_at IS NULL AND h.remna_missing_at IS NULL AND {h} = 4 \
             AND NOT EXISTS (SELECT 1 FROM subscriptions g WHERE g.client_id = h.client_id \
             AND g.ended_at IS NULL AND g.remna_missing_at IS NULL AND {g} = 3))",
            h = rank_of("h"),
            g = rank_of("g")
        ),
        Audience::NeverConnected => format!("{s} = 'active' AND s.first_connected_at IS NULL"),
        Audience::Trial => format!("{s} = 'active' AND s.kind = 'trial'"),
        Audience::NoSubscription => {
            "NOT EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id)".into()
        }
        Audience::Tariff { .. } => format!(
            "EXISTS (SELECT 1 FROM subscriptions h WHERE h.client_id = c.id AND h.ended_at IS NULL \
             AND h.remna_missing_at IS NULL AND {} = 'active' AND h.tariff_id = $3)",
            status_of("h")
        ),
        Audience::Filter(_) => "FALSE".into(),
    }
}

fn identity_match(lane: Lane) -> &'static str {
    match lane {
        Lane::Bot => "i.kind = 'telegram' AND i.value ~ '^[0-9]{1,15}$'",
        Lane::Email => {
            "i.kind = 'email' AND i.value LIKE '%_@_%' AND (i.verified OR NOT i.meta ? 'no_code')"
        }
    }
}

fn identity(lane: Lane, value: bool) -> String {
    if value {
        format!(
            "JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id AND {} \
             ORDER BY i.id LIMIT 1) ident ON true",
            identity_match(lane)
        )
    } else {
        String::new()
    }
}

const fn active_only(audience: &Audience) -> bool {
    matches!(
        audience,
        Audience::Active | Audience::Expiring { .. } | Audience::NeverConnected | Audience::Trial
    )
}

const fn needs_value(lane: Lane) -> bool {
    matches!(lane, Lane::Email)
}

const fn checks_identity(lane: Lane, value: bool) -> bool {
    !value || matches!(lane, Lane::Email)
}

fn excluded(lane: Lane) -> &'static str {
    match lane {
        Lane::Bot => "c.bot_unreachable",
        Lane::Email => {
            "EXISTS (SELECT 1 FROM broadcast_unsubscribes u WHERE u.email = lower(ident.value))"
        }
    }
}

fn audience_from(audience: &Audience, lane: Lane, value: bool) -> String {
    let subscription = if active_only(audience) {
        format!(
            "LEFT JOIN LATERAL (SELECT x.* FROM subscriptions x WHERE x.client_id = c.id AND x.ended_at IS NULL \
             AND x.remna_missing_at IS NULL ORDER BY {}, x.expire_at DESC NULLS LAST, x.id DESC LIMIT 1) s ON true",
            rank_of("x")
        )
    } else {
        String::new()
    };
    let reachable = if checks_identity(lane, value) {
        format!(
            "AND EXISTS (SELECT 1 FROM client_identities i WHERE i.client_id = c.id AND {}) ",
            identity_match(lane)
        )
    } else {
        String::new()
    };
    let active = if active_only(audience) {
        format!(
            "AND c.id IN (SELECT y.client_id FROM subscriptions y WHERE y.ended_at IS NULL \
             AND y.remna_missing_at IS NULL AND {} = 'active') ",
            status_of("y")
        )
    } else {
        String::new()
    };
    format!(
        "FROM clients c {} {subscription} \
         WHERE c.deleted_at IS NULL AND c.status = 'active' \
         {reachable}{active}AND {}",
        identity(lane, value),
        condition(audience)
    )
}

fn bound(audience: &Audience, now: DateTime<Utc>) -> DateTime<Utc> {
    match audience {
        Audience::Expiring { days } => now + Duration::days(i64::from(*days)),
        _ => now,
    }
}

async fn count_lane(
    db: &mut impl Db,
    audience: &Audience,
    lane: Lane,
    now: DateTime<Utc>,
) -> StoreResult<(i64, i64)> {
    Ok(sqlx::query_as(&format!(
        "SELECT count(*) FILTER (WHERE NOT ({skip})), count(*) FILTER (WHERE {skip}) {from}",
        skip = excluded(lane),
        from = audience_from(audience, lane, needs_value(lane))
    ))
    .bind(now)
    .bind(bound(audience, now))
    .bind(audience.tariff().map_or(0, |t| t.0))
    .fetch_one(db.conn())
    .await?)
}

fn filter_from(
    conditions: &ClientConditions,
    lane: Lane,
    value: bool,
    args: &mut SqlArgs,
    now: DateTime<Utc>,
    currency: Currency,
) -> StoreResult<String> {
    let order = format!("{}, x.expire_at DESC NULLS LAST, x.id DESC", rank_of("x"));
    let narrowed = !conditions.tariff.is_empty()
        || !conditions.category.is_empty()
        || conditions.ends.is_some()
        || conditions.ended.is_some()
        || matches!(conditions.status, Some(status) if status != ClientSegment::NoSubscription);
    let wide = !narrowed
        && (conditions.autorenew == Some(false)
            || conditions.status == Some(ClientSegment::NoSubscription));
    let subscription = if wide {
        format!(
            "LEFT JOIN (SELECT DISTINCT ON (x.client_id) x.* FROM subscriptions x \
             WHERE x.ended_at IS NULL AND x.remna_missing_at IS NULL \
             ORDER BY x.client_id, {order}) s ON s.client_id = c.id"
        )
    } else if conditions.needs_primary() {
        format!(
            "LEFT JOIN LATERAL (SELECT x.* FROM subscriptions x WHERE x.client_id = c.id AND x.ended_at IS NULL \
             AND x.remna_missing_at IS NULL ORDER BY {order} LIMIT 1) s ON true"
        )
    } else {
        String::new()
    };
    let reachable = if checks_identity(lane, value) {
        format!(
            "AND EXISTS (SELECT 1 FROM client_identities i WHERE i.client_id = c.id AND {}) ",
            identity_match(lane)
        )
    } else {
        String::new()
    };
    let conditions = conditions.conditions(args, now, currency)?;
    let tail = if conditions.is_empty() {
        String::new()
    } else {
        format!("AND {}", conditions.join(" AND "))
    };
    Ok(format!(
        "FROM clients c {} {subscription} \
         WHERE c.deleted_at IS NULL AND c.status = 'active' {reachable}{tail}",
        identity(lane, value),
    ))
}

async fn count_filter(
    db: &mut impl Db,
    conditions: &ClientConditions,
    lane: Lane,
    now: DateTime<Utc>,
) -> StoreResult<(i64, i64)> {
    let currency = crate::catalog::installation_currency(db).await?;
    let mut args = SqlArgs::with_clock(now)?;
    let from = filter_from(
        conditions,
        lane,
        needs_value(lane),
        &mut args,
        now,
        currency,
    )?;
    let sql = format!(
        "SELECT count(*) FILTER (WHERE NOT ({skip})), count(*) FILTER (WHERE {skip}) {from}",
        skip = excluded(lane),
    );
    Ok(args
        .bind_as(sqlx::query_as(&sql).persistent(false))
        .fetch_one(db.conn())
        .await?)
}

pub async fn reach(
    db: &mut impl Db,
    audience: &Audience,
    now: DateTime<Utc>,
) -> StoreResult<Reach> {
    let ((recipients, stopped), (emails, unsubscribed)) = match audience {
        Audience::Filter(conditions) => (
            count_filter(db, conditions, Lane::Bot, now).await?,
            count_filter(db, conditions, Lane::Email, now).await?,
        ),
        _ => (
            count_lane(db, audience, Lane::Bot, now).await?,
            count_lane(db, audience, Lane::Email, now).await?,
        ),
    };
    Ok(Reach {
        recipients,
        stopped,
        emails,
        unsubscribed,
    })
}

fn recipient_select(lane: Lane, id: &str) -> String {
    match lane {
        Lane::Bot => format!("SELECT {id}, 'bot', c.id, ident.value::bigint, NULL, NULL"),
        Lane::Email => {
            format!("SELECT {id}, 'email', c.id, NULL, lower(ident.value), gen_random_uuid()")
        }
    }
}

pub async fn launch(
    db: &mut impl Db,
    id: i64,
    audience: &Audience,
    channel: Channel,
    now: DateTime<Utc>,
    admin: Option<AdminId>,
) -> StoreResult<i64> {
    let mut inserted = 0u64;
    let currency = match audience {
        Audience::Filter(conditions) if conditions.uses_money() => {
            crate::catalog::installation_currency(db).await?
        }
        _ => Currency::default(),
    };
    for lane in channel.lanes() {
        inserted += match audience {
            Audience::Filter(conditions) => {
                let mut args = SqlArgs::with_clock(now)?;
                let id_arg = args.push(SqlArg::Int(id));
                let from = filter_from(conditions, lane, true, &mut args, now, currency)?;
                let sql = format!(
                    "INSERT INTO broadcast_recipients (broadcast_id, channel, client_id, chat_id, email, unsubscribe) \
                     {select} {from} AND NOT ({skip}) ON CONFLICT DO NOTHING",
                    select = recipient_select(lane, &format!("{id_arg}::bigint")),
                    skip = excluded(lane),
                );
                args.bind(sqlx::query(&sql).persistent(false))
                    .execute(db.conn())
                    .await?
                    .rows_affected()
            }
            _ => sqlx::query(&format!(
                "INSERT INTO broadcast_recipients (broadcast_id, channel, client_id, chat_id, email, unsubscribe) \
                 {select} {from} AND NOT ({skip}) ON CONFLICT DO NOTHING",
                select = recipient_select(lane, "$4"),
                from = audience_from(audience, lane, true),
                skip = excluded(lane),
            ))
            .bind(now)
            .bind(bound(audience, now))
            .bind(audience.tariff().map_or(0, |t| t.0))
            .bind(id)
            .execute(db.conn())
            .await?
            .rows_affected(),
        };
    }
    let total = i64::try_from(inserted).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE broadcasts SET status = 'sending', started_at = $2, total = $3, notice = NULL, \
         mail_notice = NULL, started_by = COALESCE($4, started_by) WHERE id = $1",
    )
    .bind(id)
    .bind(now)
    .bind(i32::try_from(total).unwrap_or(i32::MAX))
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await?;
    Ok(total)
}

pub async fn finish_empty(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'done', finished_at = now(), \
         notice = 'Получателей не нашлось: в аудитории некому отправить' WHERE id = $1",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn schedule(
    db: &mut impl Db,
    id: i64,
    at: DateTime<Utc>,
    admin: Option<AdminId>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'scheduled', send_at = $2, started_by = $3, notice = NULL, \
         mail_notice = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(at)
    .bind(admin.map(|a| a.0))
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn unschedule(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcasts SET status = 'draft', send_at = NULL, started_by = NULL, notice = NULL, \
         mail_notice = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn stop(db: &mut impl Db, id: i64, notice: Option<&str>) -> StoreResult<()> {
    sqlx::query(
        "UPDATE broadcast_recipients SET state = 'skipped', done_at = now() \
         WHERE broadcast_id = $1 AND state = 'pending'",
    )
    .bind(id)
    .execute(db.conn())
    .await?;
    sqlx::query(
        "UPDATE broadcasts SET status = 'canceled', finished_at = now(), notice = $2, mail_notice = NULL \
         WHERE id = $1",
    )
    .bind(id)
    .bind(notice)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn image_exists(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM broadcast_images WHERE id = $1)")
            .bind(id)
            .fetch_one(db.conn())
            .await?,
    )
}

pub async fn insert_image(
    db: &mut impl Db,
    bytes: &[u8],
    info: ImageInfo,
    admin: Option<AdminId>,
) -> StoreResult<ImageMeta> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO broadcast_images (content_type, bytes, width, height, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(info.kind.content_type())
    .bind(bytes)
    .bind(i32::try_from(info.width).unwrap_or(i32::MAX))
    .bind(i32::try_from(info.height).unwrap_or(i32::MAX))
    .bind(admin.map(|a| a.0))
    .fetch_one(db.conn())
    .await?;
    image_meta(db, id)
        .await?
        .ok_or(StoreError::NotFound("картинка рассылки"))
}

const IMAGE_COLUMNS: &str = "id, content_type, bytes, file_bot, file_id, public_key";

pub async fn image(db: &mut impl Db, id: i64) -> StoreResult<ImageData> {
    sqlx::query_as::<_, ImageData>(&format!(
        "SELECT {IMAGE_COLUMNS} FROM broadcast_images WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("картинка рассылки"))
}

pub async fn image_by_key(db: &mut impl Db, key: Uuid) -> StoreResult<Option<ImageData>> {
    Ok(sqlx::query_as::<_, ImageData>(&format!(
        "SELECT {IMAGE_COLUMNS} FROM broadcast_images WHERE public_key = $1"
    ))
    .bind(key)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn image_key(db: &mut impl Db, id: i64) -> StoreResult<Uuid> {
    sqlx::query_scalar("SELECT public_key FROM broadcast_images WHERE id = $1")
        .bind(id)
        .fetch_optional(db.conn())
        .await?
        .ok_or(StoreError::NotFound("картинка рассылки"))
}

pub async fn remember_file(db: &mut impl Db, id: i64, bot: i64, file_id: &str) -> StoreResult<()> {
    sqlx::query("UPDATE broadcast_images SET file_bot = $2, file_id = $3 WHERE id = $1")
        .bind(id)
        .bind(bot)
        .bind(file_id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn forget_file(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query("UPDATE broadcast_images SET file_bot = NULL, file_id = NULL WHERE id = $1")
        .bind(id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn prune_images(db: &mut impl Db, older_than_hours: i64) -> StoreResult<u64> {
    Ok(sqlx::query(
        "DELETE FROM broadcast_images i WHERE i.created_at < now() - make_interval(hours => $1::int) \
         AND NOT EXISTS (SELECT 1 FROM broadcasts b WHERE b.image_id = i.id) \
         AND NOT EXISTS (SELECT 1 FROM auto_broadcasts a WHERE a.image_id = i.id)",
    )
    .bind(i32::try_from(older_than_hours).unwrap_or(i32::MAX))
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn chat_of(db: &mut impl Db, client_id: ClientId) -> StoreResult<Option<i64>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM clients WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(client_id.0)
    .fetch_one(db.conn())
    .await?;
    if !exists {
        return Err(StoreError::NotFound("клиент"));
    }
    let value: Option<String> = sqlx::query_scalar(
        "SELECT value FROM client_identities WHERE client_id = $1 AND kind = 'telegram' ORDER BY id LIMIT 1",
    )
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(value.and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0))
}

pub async fn tariffs(db: &mut impl Db) -> StoreResult<Vec<super::TariffOption>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT t.id, t.name FROM tariffs t JOIN tariff_categories g ON g.id = t.category_id \
         WHERE t.deleted_at IS NULL ORDER BY g.sort, g.id, t.sort, t.id",
    )
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| super::TariffOption {
            id: TariffId(id),
            name,
        })
        .collect())
}

pub async fn tariff_alive(db: &mut impl Db, id: TariffId) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tariffs WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(id.0)
    .fetch_one(db.conn())
    .await?)
}

pub async fn enabled_modules(db: &mut impl Db) -> StoreResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT module FROM module_states WHERE enabled")
            .fetch_all(db.conn())
            .await?,
    )
}

pub struct Lease<'a> {
    pub key: &'a str,
    pub owner: &'a str,
    pub seconds: f64,
}

pub async fn take_lease(db: &mut impl Db, lease: &Lease<'_>) -> StoreResult<bool> {
    let taken: Option<bool> = sqlx::query_scalar(
        "INSERT INTO bot_state (key, value, updated_at) \
         VALUES ($1, jsonb_build_object('owner', $2::text, 'until', now() + make_interval(secs => $3)), now()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now() \
         WHERE bot_state.value ->> 'owner' = $2 OR (bot_state.value ->> 'until')::timestamptz < now() \
         RETURNING true",
    )
    .bind(lease.key)
    .bind(lease.owner)
    .bind(lease.seconds)
    .fetch_optional(db.conn())
    .await?;
    Ok(taken.unwrap_or(false))
}

pub async fn drop_lease(db: &mut impl Db, key: &str, owner: &str) -> StoreResult<()> {
    sqlx::query("DELETE FROM bot_state WHERE key = $1 AND value ->> 'owner' = $2")
        .bind(key)
        .bind(owner)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn abandon_flights(
    db: &mut impl Db,
    lane: Lane,
    stale: Option<(i64, i64)>,
) -> StoreResult<u64> {
    let (broadcast, seconds) = stale.map_or((None, 0), |(id, seconds)| (Some(id), seconds));
    Ok(sqlx::query(
        "UPDATE broadcast_recipients SET state = 'failed', done_at = now(), \
         error = 'Отправка прервана перезапуском: дошло ли сообщение, неизвестно, повторно не отправляли' \
         WHERE state = 'sending' AND channel = $3 AND ($1::bigint IS NULL \
             OR (broadcast_id = $1 AND claimed_at < now() - make_interval(secs => $2::double precision)))",
    )
    .bind(broadcast)
    .bind(seconds)
    .bind(lane.as_str())
    .execute(db.conn())
    .await?
    .rows_affected())
}

pub async fn due(db: &mut impl Db, limit: i64) -> StoreResult<Vec<(i64, Audience, Channel)>> {
    let rows: Vec<(i64, serde_json::Value, String)> = sqlx::query_as(
        "SELECT id, audience, channel FROM broadcasts WHERE status = 'scheduled' AND send_at <= now() \
         ORDER BY send_at, id LIMIT $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(limit)
    .fetch_all(db.conn())
    .await?;
    rows.into_iter()
        .map(|(id, audience, channel)| {
            let audience = serde_json::from_value(audience)
                .map_err(|e| StoreError::Decode(format!("аудитория рассылки: {e}")))?;
            let channel = Channel::parse(&channel)
                .ok_or_else(|| StoreError::Decode(format!("канал рассылки: {channel}")))?;
            Ok((id, audience, channel))
        })
        .collect()
}

pub async fn next_sending(
    db: &mut impl Db,
    lane: Lane,
    manual_only: bool,
) -> StoreResult<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT b.id FROM broadcasts b WHERE b.status = 'sending' \
         AND (NOT $2 OR (b.rule_id IS NULL AND b.contest_message_id IS NULL)) AND EXISTS ( \
             SELECT 1 FROM broadcast_recipients r WHERE r.broadcast_id = b.id AND r.channel = $1 \
             AND r.state IN ('pending', 'sending')) \
         ORDER BY b.started_at, b.id LIMIT 1",
    )
    .bind(lane.as_str())
    .bind(manual_only)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn automatic(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT rule_id IS NOT NULL OR contest_message_id IS NOT NULL FROM broadcasts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .unwrap_or(false))
}

pub struct Message {
    pub subject: Option<String>,
    pub text: String,
    pub buttons: Vec<ButtonSpec>,
    pub image: Option<ImageData>,
}

pub async fn message(db: &mut impl Db, id: i64, bytes: bool) -> StoreResult<Message> {
    let row: Option<(Option<String>, String, serde_json::Value, Option<i64>)> =
        sqlx::query_as("SELECT subject, text, buttons, image_id FROM broadcasts WHERE id = $1")
            .bind(id)
            .fetch_optional(db.conn())
            .await?;
    let (subject, text, buttons, image_id) = row.ok_or(StoreError::NotFound("рассылка"))?;
    let buttons = serde_json::from_value(buttons)
        .map_err(|e| StoreError::Decode(format!("кнопки рассылки: {e}")))?;
    let image = match image_id {
        Some(image_id) if bytes => Some(image(db, image_id).await?),
        Some(image_id) => Some(ImageData {
            id: image_id,
            content_type: String::new(),
            bytes: Vec::new(),
            file_bot: None,
            file_id: None,
            public_key: image_key(db, image_id).await?,
        }),
        None => None,
    };
    Ok(Message {
        subject,
        text,
        buttons,
        image,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub client_id: ClientId,
    pub chat_id: Option<i64>,
    pub email: Option<String>,
    pub unsubscribe: Option<Uuid>,
    pub attempts: i32,
    pub locale: Locale,
}

type ClaimRow = (i64, Option<i64>, Option<String>, Option<Uuid>, i32, String);

pub async fn client_locale(db: &mut impl Db, client: ClientId) -> StoreResult<Locale> {
    let raw: Option<String> = sqlx::query_scalar(
        "SELECT CASE WHEN c.locale = 'en' \
                 AND EXISTS (SELECT 1 FROM settings s WHERE s.key = 'core.languages' AND s.value->>'english' = 'true') \
                THEN 'en' ELSE 'ru' END \
         FROM clients c WHERE c.id = $1",
    )
    .bind(client.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(raw
        .and_then(|value| Locale::parse(&value).ok())
        .unwrap_or_default())
}

pub async fn claim(db: &mut impl Db, id: i64, lane: Lane, limit: i64) -> StoreResult<Vec<Claimed>> {
    let rows: Vec<ClaimRow> = sqlx::query_as(
        "WITH pick AS MATERIALIZED ( \
             SELECT q.client_id FROM broadcast_recipients q \
             WHERE q.broadcast_id = $1 AND q.channel = $3 AND q.state = 'pending' \
               AND (q.not_before IS NULL OR q.not_before <= now()) \
             ORDER BY q.client_id LIMIT $2 FOR UPDATE SKIP LOCKED) \
         UPDATE broadcast_recipients r SET state = 'sending', attempts = r.attempts + 1, claimed_at = now() \
         FROM pick WHERE r.broadcast_id = $1 AND r.channel = $3 AND r.client_id = pick.client_id \
           AND EXISTS (SELECT 1 FROM broadcasts b WHERE b.id = $1 AND b.status = 'sending') \
         RETURNING r.client_id, r.chat_id, r.email, r.unsubscribe, r.attempts, \
           CASE WHEN EXISTS (SELECT 1 FROM clients c WHERE c.id = r.client_id AND c.locale = 'en') \
                 AND EXISTS (SELECT 1 FROM settings s WHERE s.key = 'core.languages' AND s.value->>'english' = 'true') \
                THEN 'en' ELSE 'ru' END",
    )
    .bind(id)
    .bind(limit)
    .bind(lane.as_str())
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(client_id, chat_id, email, unsubscribe, attempts, locale)| Claimed {
                client_id: ClientId(client_id),
                chat_id,
                email,
                unsubscribe,
                attempts,
                locale: Locale::parse(&locale).unwrap_or_default(),
            },
        )
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome<'a> {
    Sent,
    Blocked(&'a str),
    Retry {
        after_ms: i64,
        error: Option<&'a str>,
        counted: bool,
    },
    Failed(&'a str),
}

pub async fn record(
    db: &mut impl Db,
    id: i64,
    lane: Lane,
    client_id: ClientId,
    outcome: &Outcome<'_>,
) -> StoreResult<bool> {
    let channel = lane.as_str();
    let done = match outcome {
        Outcome::Sent => {
            sqlx::query(
                "UPDATE broadcast_recipients SET state = 'sent', done_at = now(), error = NULL \
                 WHERE broadcast_id = $1 AND client_id = $2 AND channel = $3 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(channel)
            .execute(db.conn())
            .await?
        }
        Outcome::Blocked(error) => {
            if lane == Lane::Bot {
                sqlx::query("UPDATE clients SET bot_unreachable = true WHERE id = $1")
                    .bind(client_id.0)
                    .execute(db.conn())
                    .await?;
            }
            sqlx::query(
                "UPDATE broadcast_recipients SET state = 'blocked', done_at = now(), error = $4 \
                 WHERE broadcast_id = $1 AND client_id = $2 AND channel = $3 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(channel)
            .bind(error)
            .execute(db.conn())
            .await?
        }
        Outcome::Retry {
            after_ms,
            error,
            counted,
        } => {
            sqlx::query(
                "UPDATE broadcast_recipients SET \
                 state = CASE WHEN (SELECT b.status FROM broadcasts b WHERE b.id = $1) = 'sending' \
                     THEN 'pending' ELSE 'skipped' END, \
                 not_before = now() + make_interval(secs => $4::double precision / 1000), \
                 error = COALESCE($5, error), \
                 attempts = CASE WHEN $6 THEN attempts ELSE greatest(attempts - 1, 0) END, \
                 done_at = CASE WHEN (SELECT b.status FROM broadcasts b WHERE b.id = $1) = 'sending' \
                     THEN NULL ELSE now() END \
                 WHERE broadcast_id = $1 AND client_id = $2 AND channel = $3 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(channel)
            .bind(*after_ms)
            .bind(*error)
            .bind(*counted)
            .execute(db.conn())
            .await?
        }
        Outcome::Failed(error) => {
            sqlx::query(
                "UPDATE broadcast_recipients SET state = 'failed', done_at = now(), error = $4 \
                 WHERE broadcast_id = $1 AND client_id = $2 AND channel = $3 AND state = 'sending'",
            )
            .bind(id)
            .bind(client_id.0)
            .bind(channel)
            .bind(error)
            .execute(db.conn())
            .await?
        }
    };
    Ok(done.rows_affected() > 0)
}

const FINISHED: &str = "status = 'sending' AND NOT EXISTS ( \
     SELECT 1 FROM broadcast_recipients r WHERE r.broadcast_id = broadcasts.id AND r.state IN ('pending', 'sending'))";

pub async fn complete(db: &mut impl Db, id: i64) -> StoreResult<bool> {
    let done = sqlx::query(&format!(
        "UPDATE broadcasts SET status = 'done', finished_at = now(), notice = NULL, mail_notice = NULL \
         WHERE id = $1 AND {FINISHED}"
    ))
    .bind(id)
    .execute(db.conn())
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn complete_finished(db: &mut impl Db) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(&format!(
        "UPDATE broadcasts SET status = 'done', finished_at = now(), notice = NULL, mail_notice = NULL \
         WHERE {FINISHED} RETURNING id"
    ))
    .fetch_all(db.conn())
    .await?)
}

pub async fn status(db: &mut impl Db, id: i64) -> StoreResult<Option<Status>> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM broadcasts WHERE id = $1")
        .bind(id)
        .fetch_optional(db.conn())
        .await?;
    Ok(status.as_deref().and_then(Status::parse))
}

pub async fn next_retry_ms(db: &mut impl Db, id: i64, lane: Lane) -> StoreResult<Option<i64>> {
    let wait: Option<f64> = sqlx::query_scalar(
        "SELECT (extract(epoch FROM (min(not_before) - now())) * 1000)::double precision FROM broadcast_recipients \
         WHERE broadcast_id = $1 AND channel = $2 AND state = 'pending'",
    )
    .bind(id)
    .bind(lane.as_str())
    .fetch_one(db.conn())
    .await?;
    Ok(wait.map(|ms| ms.max(0.0).round() as i64))
}

pub async fn set_notice(
    db: &mut impl Db,
    id: i64,
    lane: Lane,
    notice: Option<&str>,
) -> StoreResult<()> {
    let column = match lane {
        Lane::Bot => "notice",
        Lane::Email => "mail_notice",
    };
    sqlx::query(&format!(
        "UPDATE broadcasts SET {column} = $2 WHERE id = $1 AND status = 'sending' \
         AND {column} IS DISTINCT FROM $2"
    ))
    .bind(id)
    .bind(notice)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn sent_count(db: &mut impl Db, id: i64, lane: Lane) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM broadcast_recipients WHERE broadcast_id = $1 AND channel = $2 AND state = 'sent'",
    )
    .bind(id)
    .bind(lane.as_str())
    .fetch_one(db.conn())
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    pub email: String,
    pub client_id: ClientId,
    pub broadcast_id: i64,
    pub unsubscribed: bool,
}

pub async fn subscriber(db: &mut impl Db, token: Uuid) -> StoreResult<Option<Subscriber>> {
    let row: Option<(String, i64, i64, bool)> = sqlx::query_as(
        "SELECT r.email, r.client_id, r.broadcast_id, \
         EXISTS (SELECT 1 FROM broadcast_unsubscribes u WHERE u.email = r.email) \
         FROM broadcast_recipients r WHERE r.unsubscribe = $1 AND r.channel = 'email'",
    )
    .bind(token)
    .fetch_optional(db.conn())
    .await?;
    Ok(row.map(
        |(email, client_id, broadcast_id, unsubscribed)| Subscriber {
            email,
            client_id: ClientId(client_id),
            broadcast_id,
            unsubscribed,
        },
    ))
}

pub async fn unsubscribe(db: &mut impl Db, who: &Subscriber) -> StoreResult<bool> {
    Ok(sqlx::query(
        "INSERT INTO broadcast_unsubscribes (email, client_id, broadcast_id) VALUES ($1, $2, $3) \
         ON CONFLICT (email) DO NOTHING",
    )
    .bind(&who.email)
    .bind(who.client_id.0)
    .bind(who.broadcast_id)
    .execute(db.conn())
    .await?
    .rows_affected()
        > 0)
}

pub async fn resubscribe(db: &mut impl Db, email: &str) -> StoreResult<bool> {
    Ok(
        sqlx::query("DELETE FROM broadcast_unsubscribes WHERE email = $1")
            .bind(email)
            .execute(db.conn())
            .await?
            .rows_affected()
            > 0,
    )
}

pub async fn brand(db: &mut impl Db) -> StoreResult<String> {
    Ok(db
        .settings()
        .get(sn_domain::settings::keys::BRAND_NAME)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "STEALTHNET".into()))
}
