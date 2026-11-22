use super::{Audience, Content, EntryFilter, ListFilter, MessageContent};
use crate::catalog::antibot::Invite;
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::paging::{Page, PageRequest};
use sn_store::{Db, StoreError, StoreResult};

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ContestRow {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub prize: serde_json::Value,
    pub winners: i32,
    pub conditions: serde_json::Value,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub seed: Vec<u8>,
    pub seed_hash: String,
    pub list_hash: Option<String>,
    pub entries_drawn: Option<i32>,
    pub drawn_at: Option<DateTime<Utc>>,
    pub canceled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "c.id, c.title, c.description, c.prize, c.winners, c.conditions, c.starts_at, c.ends_at, \
                       c.seed, c.seed_hash, c.list_hash, c.entries_drawn, c.drawn_at, c.canceled_at, c.created_at, \
                       c.updated_at";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, sqlx::FromRow)]
pub struct Counts {
    pub entries: i64,
    pub excluded: i64,
    pub issued: i64,
    pub waiting: i64,
    pub failed: i64,
    pub pending: i64,
}

const COUNTS: &str = "(SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id) AS entries, \
                      (SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id AND e.excluded IS NOT NULL) AS excluded, \
                      (SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id AND e.prize_state = 'issued') AS issued, \
                      (SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id AND e.prize_state = 'waiting') AS waiting, \
                      (SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id AND e.prize_state = 'failed') AS failed, \
                      (SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id AND e.prize_state IN ('pending', 'issuing')) AS pending";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ListRow {
    #[sqlx(flatten)]
    pub contest: ContestRow,
    #[sqlx(flatten)]
    pub counts: Counts,
}

pub async fn insert(
    db: &mut impl Db,
    content: &Content,
    seed: &[u8],
    seed_hash: &str,
    by: Option<AdminId>,
) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO contests (title, description, prize, winners, conditions, starts_at, ends_at, seed, seed_hash, \
         created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(&content.title)
    .bind(content.description.as_deref())
    .bind(serde_json::to_value(&content.prize).map_err(|e| StoreError::Invalid(e.to_string()))?)
    .bind(i32::try_from(content.winners).unwrap_or(i32::MAX))
    .bind(serde_json::to_value(&content.conditions).map_err(|e| StoreError::Invalid(e.to_string()))?)
    .bind(content.starts_at)
    .bind(content.ends_at)
    .bind(seed)
    .bind(seed_hash)
    .bind(by.map(|a| a.0))
    .fetch_one(db.conn())
    .await?)
}

pub async fn update(db: &mut impl Db, id: i64, content: &Content) -> StoreResult<()> {
    sqlx::query(
        "UPDATE contests SET title = $2, description = $3, prize = $4, winners = $5, conditions = $6, \
         starts_at = $7, ends_at = $8 WHERE id = $1",
    )
    .bind(id)
    .bind(&content.title)
    .bind(content.description.as_deref())
    .bind(serde_json::to_value(&content.prize).map_err(|e| StoreError::Invalid(e.to_string()))?)
    .bind(i32::try_from(content.winners).unwrap_or(i32::MAX))
    .bind(serde_json::to_value(&content.conditions).map_err(|e| StoreError::Invalid(e.to_string()))?)
    .bind(content.starts_at)
    .bind(content.ends_at)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn row(db: &mut impl Db, id: i64) -> StoreResult<ContestRow> {
    sqlx::query_as::<_, ContestRow>(&format!("SELECT {COLUMNS} FROM contests c WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(db.conn())
        .await?
        .ok_or(StoreError::NotFound("конкурс"))
}

pub async fn lock(db: &mut impl Db, id: i64) -> StoreResult<ContestRow> {
    sqlx::query_as::<_, ContestRow>(&format!(
        "SELECT {COLUMNS} FROM contests c WHERE c.id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("конкурс"))
}

pub async fn share(db: &mut impl Db, id: i64) -> StoreResult<ContestRow> {
    sqlx::query_as::<_, ContestRow>(&format!(
        "SELECT {COLUMNS} FROM contests c WHERE c.id = $1 FOR SHARE"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?
    .ok_or(StoreError::NotFound("конкурс"))
}

pub async fn counts(db: &mut impl Db, id: i64) -> StoreResult<Counts> {
    Ok(
        sqlx::query_as::<_, Counts>(&format!("SELECT {COUNTS} FROM contests c WHERE c.id = $1"))
            .bind(id)
            .fetch_optional(db.conn())
            .await?
            .unwrap_or_default(),
    )
}

pub async fn list(
    db: &mut impl Db,
    filter: ListFilter,
    page: PageRequest,
) -> StoreResult<Page<ListRow>> {
    let condition = match filter {
        ListFilter::All => "true",
        ListFilter::Live => "c.canceled_at IS NULL AND c.drawn_at IS NULL",
        ListFilter::Done => "(c.canceled_at IS NOT NULL OR c.drawn_at IS NOT NULL)",
    };
    let rows: Vec<ListRow> = sqlx::query_as(&format!(
        "SELECT {COLUMNS}, {COUNTS} FROM contests c WHERE {condition} \
         AND ($1::bigint IS NULL OR c.id < $1) ORDER BY c.id DESC LIMIT $2"
    ))
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    Ok(Page::from_overfetch(rows, page, |r| r.contest.id))
}

pub async fn delete(db: &mut impl Db, id: i64) -> StoreResult<()> {
    sqlx::query("DELETE FROM contests WHERE id = $1")
        .bind(id)
        .execute(db.conn())
        .await?;
    Ok(())
}

pub async fn cancel(db: &mut impl Db, id: i64, now: DateTime<Utc>) -> StoreResult<()> {
    sqlx::query("UPDATE contests SET canceled_at = $2 WHERE id = $1 AND canceled_at IS NULL")
        .bind(id)
        .bind(now)
        .execute(db.conn())
        .await?;
    sqlx::query(
        "UPDATE contest_messages SET queued_at = $2, recipients = 0, skipped = 'Конкурс отменён' \
         WHERE contest_id = $1 AND queued_at IS NULL",
    )
    .bind(id)
    .bind(now)
    .execute(db.conn())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct MessageRow {
    pub id: i64,
    pub contest_id: i64,
    pub audience: String,
    pub only_eligible: bool,
    pub send_at: Option<DateTime<Utc>>,
    pub text: String,
    pub queued_at: Option<DateTime<Utc>>,
    pub recipients: Option<i32>,
    pub skipped: Option<String>,
}

const MESSAGE_COLUMNS: &str = "m.id, m.contest_id, m.audience, m.only_eligible, m.send_at, m.text, m.queued_at, m.recipients, m.skipped";

pub async fn messages(db: &mut impl Db, contest_id: i64) -> StoreResult<Vec<MessageRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM contest_messages m WHERE m.contest_id = $1 \
         ORDER BY m.send_at NULLS LAST, m.id"
    ))
    .bind(contest_id)
    .fetch_all(db.conn())
    .await?)
}

pub async fn replace_messages(
    db: &mut impl Db,
    contest_id: i64,
    keep: &[MessageContent],
) -> StoreResult<()> {
    let kept: Vec<i64> = keep.iter().filter_map(|m| m.id).collect();
    sqlx::query(
        "DELETE FROM contest_messages WHERE contest_id = $1 AND queued_at IS NULL AND NOT (id = ANY($2))",
    )
    .bind(contest_id)
    .bind(&kept)
    .execute(db.conn())
    .await?;
    for message in keep {
        match message.id {
            Some(id) => {
                sqlx::query(
                    "UPDATE contest_messages SET audience = $3, only_eligible = $4, send_at = $5, text = $6 \
                     WHERE id = $1 AND contest_id = $2 AND queued_at IS NULL",
                )
                .bind(id)
                .bind(contest_id)
                .bind(message.audience.as_str())
                .bind(message.only_eligible)
                .bind(message.send_at)
                .bind(&message.text)
                .execute(db.conn())
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO contest_messages (contest_id, audience, only_eligible, send_at, text) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(contest_id)
                .bind(message.audience.as_str())
                .bind(message.only_eligible)
                .bind(message.send_at)
                .bind(&message.text)
                .execute(db.conn())
                .await?;
            }
        }
    }
    Ok(())
}

pub async fn due_messages(db: &mut impl Db, now: DateTime<Utc>) -> StoreResult<Vec<MessageRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM contest_messages m JOIN contests c ON c.id = m.contest_id \
         WHERE m.queued_at IS NULL AND ( \
             (m.audience <> 'results' AND m.send_at <= $1) \
             OR (m.audience = 'results' AND c.drawn_at IS NOT NULL) \
             OR c.canceled_at IS NOT NULL) \
         ORDER BY m.send_at NULLS LAST, m.id LIMIT 50"
    ))
    .bind(now)
    .fetch_all(db.conn())
    .await?)
}

pub async fn lock_message(db: &mut impl Db, id: i64) -> StoreResult<Option<MessageRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM contest_messages m WHERE m.id = $1 AND m.queued_at IS NULL FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn mark_message(
    db: &mut impl Db,
    id: i64,
    now: DateTime<Utc>,
    recipients: i64,
    skipped: Option<&str>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE contest_messages SET queued_at = $2, recipients = $3, skipped = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(now)
    .bind(i32::try_from(recipients).unwrap_or(i32::MAX))
    .bind(skipped)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn audience_chats(
    db: &mut impl Db,
    contest_id: i64,
    audience: Audience,
) -> StoreResult<Vec<(ClientId, i64)>> {
    let filter = match audience {
        Audience::Outsiders => {
            "NOT EXISTS (SELECT 1 FROM contest_entries e WHERE e.contest_id = $1 AND e.client_id = c.id)"
        }
        Audience::Participants => {
            "EXISTS (SELECT 1 FROM contest_entries e WHERE e.contest_id = $1 AND e.client_id = c.id)"
        }
        Audience::Results => {
            "EXISTS (SELECT 1 FROM contest_entries e WHERE e.contest_id = $1 AND e.client_id = c.id \
             AND e.place IS NULL)"
        }
    };
    let rows: Vec<(i64, String)> = sqlx::query_as(&format!(
        "SELECT c.id, tg.value FROM clients c \
         JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id AND i.kind = 'telegram' \
             AND i.value ~ '^[0-9]{{1,15}}$' ORDER BY i.id LIMIT 1) tg ON true \
         WHERE c.deleted_at IS NULL AND c.status = 'active' AND NOT c.bot_unreachable AND {filter} \
         ORDER BY c.id"
    ))
    .bind(contest_id)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, chat)| {
            chat.parse::<i64>()
                .ok()
                .filter(|c| *c > 0)
                .map(|c| (ClientId(id), c))
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct FactsRow {
    pub id: i64,
    pub blocked: bool,
    pub subscribed: bool,
    pub purchased: bool,
    pub friends: i64,
}

pub struct FactsQuery {
    pub at: DateTime<Utc>,
    pub paid_only: bool,
    pub purchase_from: DateTime<Utc>,
    pub purchase_until: DateTime<Utc>,
    pub purchase_min: i64,
    pub friends_from: DateTime<Utc>,
    pub friends_until: DateTime<Utc>,
    pub friends_paid: bool,
}

const PAID_ORDER: &str = "o.paid_at IS NOT NULL AND o.status IN ('paid', 'fulfilled', 'review') \
                          AND o.kind IN ('new', 'renewal', 'change', 'module')";

pub async fn facts(
    db: &mut impl Db,
    ids: &[i64],
    query: &FactsQuery,
) -> StoreResult<Vec<FactsRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let friend_paid = PAID_ORDER.replace("o.", "f.");
    Ok(sqlx::query_as(&format!(
        "SELECT c.id, (c.status <> 'active' OR c.deleted_at IS NOT NULL) AS blocked, \
         EXISTS (SELECT 1 FROM subscriptions s WHERE s.client_id = c.id AND s.remna_user_id IS NOT NULL \
             AND s.created_at <= $2 AND (s.ended_at IS NULL OR s.ended_at > $2) AND s.expire_at > $2 \
             AND (s.remna_missing_at IS NULL OR s.remna_missing_at > $2) \
             AND (s.status IS NULL OR s.status <> 'disabled') AND (NOT $3 OR s.kind IN ('paid', 'gift'))) AS subscribed, \
         EXISTS (SELECT 1 FROM orders o WHERE o.client_id = c.id AND {PAID_ORDER} \
             AND o.paid_at >= $4 AND o.paid_at < $5 AND o.total_minor >= $6) AS purchased, \
         (SELECT count(*) FROM referral_links l WHERE l.referrer_id = c.id AND l.created_at >= $7 \
             AND l.created_at < $8 AND (NOT $9 OR EXISTS (SELECT 1 FROM orders f WHERE f.client_id = l.client_id \
             AND {friend_paid}))) AS friends \
         FROM clients c WHERE c.id = ANY($1) ORDER BY c.id"
    ))
    .bind(ids)
    .bind(query.at)
    .bind(query.paid_only)
    .bind(query.purchase_from)
    .bind(query.purchase_until)
    .bind(query.purchase_min)
    .bind(query.friends_from)
    .bind(query.friends_until)
    .bind(query.friends_paid)
    .fetch_all(db.conn())
    .await?)
}

pub async fn invites(
    db: &mut impl Db,
    ids: &[i64],
    query: &FactsQuery,
) -> StoreResult<Vec<Invite>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let friend_paid = PAID_ORDER.replace("o.", "f.");
    let rows: Vec<(i64, i64, DateTime<Utc>)> = sqlx::query_as(&format!(
        "SELECT l.referrer_id, l.client_id, l.created_at FROM referral_links l \
         WHERE l.referrer_id = ANY($1) AND l.created_at >= $2 AND l.created_at < $3 \
           AND (NOT $4 OR EXISTS (SELECT 1 FROM orders f WHERE f.client_id = l.client_id AND {friend_paid})) \
         ORDER BY l.referrer_id, l.created_at, l.client_id"
    ))
    .bind(ids)
    .bind(query.friends_from)
    .bind(query.friends_until)
    .bind(query.friends_paid)
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(referrer, invitee, linked_at)| Invite {
            referrer: ClientId(referrer),
            invitee: ClientId(invitee),
            linked_at,
        })
        .collect())
}

pub async fn insert_entry(
    db: &mut impl Db,
    contest_id: i64,
    client_id: ClientId,
) -> StoreResult<bool> {
    Ok(sqlx::query(
        "INSERT INTO contest_entries (contest_id, client_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(contest_id)
    .bind(client_id.0)
    .execute(db.conn())
    .await?
    .rows_affected()
        > 0)
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct EntryRow {
    pub contest_id: i64,
    pub client_id: i64,
    pub joined_at: DateTime<Utc>,
    pub excluded: Option<String>,
    pub ticket: Option<String>,
    pub place: Option<i32>,
    pub prize_state: Option<String>,
    pub prize_note: Option<String>,
    pub prize_code: Option<String>,
    pub prize_error: Option<String>,
    pub prize_at: Option<DateTime<Utc>>,
    pub notified: Option<String>,
}

const ENTRY_COLUMNS: &str = "e.contest_id, e.client_id, e.joined_at, e.excluded, e.ticket, e.place, e.prize_state, \
                             e.prize_note, e.prize_code, e.prize_error, e.prize_at, e.notified";

pub async fn entry(
    db: &mut impl Db,
    contest_id: i64,
    client_id: ClientId,
) -> StoreResult<Option<EntryRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ENTRY_COLUMNS} FROM contest_entries e WHERE e.contest_id = $1 AND e.client_id = $2"
    ))
    .bind(contest_id)
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn lock_entry(
    db: &mut impl Db,
    contest_id: i64,
    client_id: ClientId,
) -> StoreResult<Option<EntryRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ENTRY_COLUMNS} FROM contest_entries e WHERE e.contest_id = $1 AND e.client_id = $2 FOR UPDATE"
    ))
    .bind(contest_id)
    .bind(client_id.0)
    .fetch_optional(db.conn())
    .await?)
}

pub async fn entry_ids(db: &mut impl Db, contest_id: i64) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT client_id FROM contest_entries WHERE contest_id = $1 ORDER BY client_id",
    )
    .bind(contest_id)
    .fetch_all(db.conn())
    .await?)
}

pub async fn winners(db: &mut impl Db, contest_id: i64) -> StoreResult<Vec<EntryRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ENTRY_COLUMNS} FROM contest_entries e WHERE e.contest_id = $1 AND e.place IS NOT NULL \
         ORDER BY e.place"
    ))
    .bind(contest_id)
    .fetch_all(db.conn())
    .await?)
}

pub async fn entries(
    db: &mut impl Db,
    contest_id: i64,
    filter: EntryFilter,
    page: PageRequest,
) -> StoreResult<Page<EntryRow>> {
    let condition = match filter {
        EntryFilter::All => "true",
        EntryFilter::Drawn => "e.excluded IS NULL",
        EntryFilter::Excluded => "e.excluded IS NOT NULL",
        EntryFilter::Winners => "e.place IS NOT NULL",
    };
    let rows: Vec<EntryRow> = sqlx::query_as(&format!(
        "SELECT {ENTRY_COLUMNS} FROM contest_entries e WHERE e.contest_id = $1 AND {condition} \
         AND ($2::bigint IS NULL OR e.client_id > $2) ORDER BY e.place NULLS LAST, e.client_id LIMIT $3"
    ))
    .bind(contest_id)
    .bind(page.after)
    .bind(page.fetch_limit())
    .fetch_all(db.conn())
    .await?;
    Ok(Page::from_overfetch(rows, page, |r| r.client_id))
}

pub async fn all_entries(db: &mut impl Db, contest_id: i64) -> StoreResult<Vec<EntryRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ENTRY_COLUMNS} FROM contest_entries e WHERE e.contest_id = $1 ORDER BY e.client_id"
    ))
    .bind(contest_id)
    .fetch_all(db.conn())
    .await?)
}

pub async fn exclude(db: &mut impl Db, contest_id: i64, rows: &[(i64, String)]) -> StoreResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let reasons: Vec<&str> = rows.iter().map(|(_, r)| r.as_str()).collect();
    sqlx::query(
        "UPDATE contest_entries e SET excluded = t.r FROM unnest($2::bigint[], $3::text[]) AS t(c, r) \
         WHERE e.contest_id = $1 AND e.client_id = t.c",
    )
    .bind(contest_id)
    .bind(&ids)
    .bind(&reasons)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_tickets(
    db: &mut impl Db,
    contest_id: i64,
    rows: &[(i64, String, Option<i32>)],
) -> StoreResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();
    let tickets: Vec<&str> = rows.iter().map(|(_, t, _)| t.as_str()).collect();
    let places: Vec<Option<i32>> = rows.iter().map(|(_, _, p)| *p).collect();
    sqlx::query(
        "UPDATE contest_entries e SET ticket = t.k, place = t.p, \
         prize_state = CASE WHEN t.p IS NULL THEN NULL ELSE 'pending' END \
         FROM unnest($2::bigint[], $3::text[], $4::int[]) AS t(c, k, p) \
         WHERE e.contest_id = $1 AND e.client_id = t.c",
    )
    .bind(contest_id)
    .bind(&ids)
    .bind(&tickets)
    .bind(&places)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_drawn(
    db: &mut impl Db,
    id: i64,
    list_hash: &str,
    drawn: i64,
    now: DateTime<Utc>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE contests SET list_hash = $2, entries_drawn = $3, drawn_at = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(list_hash)
    .bind(i32::try_from(drawn).unwrap_or(i32::MAX))
    .bind(now)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn due_draws(db: &mut impl Db, now: DateTime<Utc>) -> StoreResult<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM contests WHERE ends_at <= $1 AND drawn_at IS NULL AND canceled_at IS NULL \
         ORDER BY ends_at LIMIT 20",
    )
    .bind(now)
    .fetch_all(db.conn())
    .await?)
}

pub async fn prizes_due(db: &mut impl Db, limit: i64) -> StoreResult<Vec<(i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT e.contest_id, e.client_id FROM contest_entries e JOIN contests c ON c.id = e.contest_id \
         WHERE c.canceled_at IS NULL AND (e.prize_state = 'pending' OR (e.prize_state = 'waiting' AND EXISTS ( \
             SELECT 1 FROM subscriptions s WHERE s.client_id = e.client_id AND s.ended_at IS NULL \
             AND s.remna_user_id IS NOT NULL AND s.remna_missing_at IS NULL \
             AND (s.status IS NULL OR s.status <> 'disabled')))) \
         ORDER BY e.contest_id, e.place LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

pub struct PrizeUpdate<'a> {
    pub state: &'a str,
    pub note: Option<&'a str>,
    pub code: Option<&'a str>,
    pub error: Option<&'a str>,
    pub at: Option<DateTime<Utc>>,
}

pub async fn set_prize(
    db: &mut impl Db,
    contest_id: i64,
    client_id: i64,
    update: &PrizeUpdate<'_>,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE contest_entries SET prize_state = $3, prize_note = COALESCE($4, prize_note), \
         prize_code = COALESCE($5, prize_code), prize_error = $6, prize_at = COALESCE($7, prize_at) \
         WHERE contest_id = $1 AND client_id = $2",
    )
    .bind(contest_id)
    .bind(client_id)
    .bind(update.state)
    .bind(update.note)
    .bind(update.code)
    .bind(update.error)
    .bind(update.at)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn set_notified(
    db: &mut impl Db,
    contest_id: i64,
    client_id: i64,
    notified: &str,
) -> StoreResult<()> {
    sqlx::query(
        "UPDATE contest_entries SET notified = $3 WHERE contest_id = $1 AND client_id = $2",
    )
    .bind(contest_id)
    .bind(client_id)
    .bind(notified)
    .execute(db.conn())
    .await?;
    Ok(())
}

pub async fn log(
    db: &mut impl Db,
    contest_id: i64,
    kind: &str,
    text: &str,
    details: serde_json::Value,
) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO contest_log (contest_id, kind, text, details) VALUES ($1, $2, $3, $4)",
    )
    .bind(contest_id)
    .bind(kind)
    .bind(text)
    .bind(details)
    .execute(db.conn())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct LogRow {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

pub async fn log_rows(db: &mut impl Db, contest_id: i64, limit: i64) -> StoreResult<Vec<LogRow>> {
    Ok(sqlx::query_as(
        "SELECT id, kind, text, details, created_at FROM contest_log WHERE contest_id = $1 \
         ORDER BY id DESC LIMIT $2",
    )
    .bind(contest_id)
    .bind(limit)
    .fetch_all(db.conn())
    .await?)
}

pub async fn open_for(
    db: &mut impl Db,
    client_id: ClientId,
    now: DateTime<Utc>,
    recent_days: i64,
) -> StoreResult<Vec<(ContestRow, bool, i64)>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        #[sqlx(flatten)]
        contest: ContestRow,
        joined: bool,
        entries: i64,
    }
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS}, EXISTS (SELECT 1 FROM contest_entries e WHERE e.contest_id = c.id AND e.client_id = $1) AS joined, \
         (SELECT count(*) FROM contest_entries e WHERE e.contest_id = c.id) AS entries \
         FROM contests c WHERE c.canceled_at IS NULL AND c.starts_at <= $2 AND ( \
             c.drawn_at IS NULL \
             OR (c.drawn_at > $2 - make_interval(days => $3::int) \
                 AND EXISTS (SELECT 1 FROM contest_entries e WHERE e.contest_id = c.id AND e.client_id = $1))) \
         ORDER BY (c.drawn_at IS NULL) DESC, c.ends_at, c.id"
    ))
    .bind(client_id.0)
    .bind(now)
    .bind(i32::try_from(recent_days).unwrap_or(7))
    .fetch_all(db.conn())
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.contest, r.joined, r.entries))
        .collect())
}

pub async fn running_count(db: &mut impl Db, now: DateTime<Utc>) -> StoreResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM contests WHERE canceled_at IS NULL AND drawn_at IS NULL \
         AND starts_at <= $1 AND ends_at > $1",
    )
    .bind(now)
    .fetch_one(db.conn())
    .await?)
}

pub async fn utc_offset_minutes(db: &mut impl Db) -> StoreResult<i64> {
    Ok(db
        .settings()
        .get(sn_domain::settings::keys::REMNA_UTC_OFFSET)
        .await?
        .and_then(|v| v.as_i64())
        .unwrap_or(0))
}
