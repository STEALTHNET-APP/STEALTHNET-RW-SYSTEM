use super::clients::display_name;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, SubscriptionId, TicketFileId, TicketId};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::support::{
    AiState, HandoffReason, MessageAuthor, Ticket, TicketAi, TicketChannel, TicketFilter,
    TicketMessage, TicketOrder, TicketStatus,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTicket {
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
    pub channel: TicketChannel,
    pub subject: Option<String>,
    pub body: String,
    pub ai: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReply {
    pub ticket_id: TicketId,
    pub author: MessageAuthor,
    pub admin_id: Option<AdminId>,
    pub body: String,
    pub attachments: Vec<TicketFileId>,
    pub close: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TicketListItem {
    pub ticket: Ticket,
    pub client_name: Option<String>,
    pub telegram_username: Option<String>,
    pub email: Option<String>,
    pub waiting_since: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub preview: Option<String>,
    pub last_author: Option<String>,
    pub unread: bool,
    pub ai: Option<TicketAi>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TicketPreview {
    pub author: MessageAuthor,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClientTicket {
    #[serde(flatten)]
    pub ticket: Ticket,
    pub preview: Option<TicketPreview>,
    pub unread: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct TicketCounts {
    pub waiting: i64,
    pub answered: i64,
    pub closed: i64,
}

impl TicketCounts {
    pub const fn get(&self, filter: TicketFilter) -> i64 {
        match filter {
            TicketFilter::Waiting => self.waiting,
            TicketFilter::Answered => self.answered,
            TicketFilter::Closed => self.closed,
        }
    }
}

const fn author_str(author: MessageAuthor) -> &'static str {
    match author {
        MessageAuthor::Client => "client",
        MessageAuthor::Support => "support",
        MessageAuthor::Ai => "ai",
        MessageAuthor::System => "system",
    }
}

fn parse_author(value: &str) -> StoreResult<MessageAuthor> {
    match value {
        "client" => Ok(MessageAuthor::Client),
        "support" => Ok(MessageAuthor::Support),
        "ai" => Ok(MessageAuthor::Ai),
        "system" => Ok(MessageAuthor::System),
        other => Err(StoreError::Decode(format!("автор сообщения: {other}"))),
    }
}

type AiRow = (Option<String>, Option<String>, Option<DateTime<Utc>>);

pub fn ticket_ai(
    state: Option<&str>,
    reason: Option<&str>,
    handoff_at: Option<DateTime<Utc>>,
) -> StoreResult<Option<TicketAi>> {
    let Some(state) = state else {
        return Ok(None);
    };
    let state = AiState::parse(state).map_err(|e| StoreError::Decode(e.to_string()))?;
    let reason = reason
        .map(HandoffReason::parse)
        .transpose()
        .map_err(|e| StoreError::Decode(e.to_string()))?;
    Ok(Some(TicketAi {
        state,
        reason,
        handoff_at,
    }))
}

fn parse_status(value: &str) -> StoreResult<TicketStatus> {
    match value {
        "open" => Ok(TicketStatus::Open),
        "closed" => Ok(TicketStatus::Closed),
        other => Err(StoreError::Decode(format!("статус обращения: {other}"))),
    }
}

#[derive(sqlx::FromRow)]
struct TicketRow {
    id: i64,
    client_id: i64,
    status: String,
    channel: String,
    subject: Option<String>,
    last_author: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "t.id, t.client_id, t.status, t.channel, t.subject, t.last_author, t.created_at, \
                       t.updated_at, t.closed_at";

const PREVIEW_CHARS: i32 = 200;

pub const UNREAD_FROM_KEY: &str = "support.unread_from";

const UNREAD: &str = "FROM ticket_messages u WHERE u.ticket_id = t.id AND u.author IN ('support', 'ai') \
                      AND u.read_at IS NULL \
                      AND u.created_at >= COALESCE((SELECT (s.value #>> '{}')::timestamptz FROM bot_state s \
                          WHERE s.key = 'support.unread_from'), 'infinity') \
                      AND u.id > COALESCE((SELECT max(c.id) FROM ticket_messages c \
                          WHERE c.ticket_id = t.id AND c.author = 'client'), 0)";

const WAITING_LONGER: &str = "t.status = 'open' AND t.last_author = 'client' \
     AND COALESCE((SELECT m.created_at FROM ticket_messages m WHERE m.id = t.last_message_id), t.created_at) \
         < now() - make_interval(days => $1::int)";

fn sorted_ids(mut ids: Vec<i64>) -> Vec<TicketId> {
    ids.sort_unstable();
    ids.into_iter().map(TicketId).collect()
}

const fn filter_condition(filter: TicketFilter) -> &'static str {
    match filter {
        TicketFilter::Waiting => "t.status = 'open' AND t.last_author = 'client'",
        TicketFilter::Answered => "t.status = 'open' AND t.last_author <> 'client'",
        TicketFilter::Closed => "t.status = 'closed'",
    }
}

const fn order_parts(order: TicketOrder) -> (&'static str, &'static str, &'static str) {
    match order {
        TicketOrder::Recent => ("COALESCE(t.last_message_at, t.created_at)", "DESC", "<"),
        TicketOrder::Longest => (
            "COALESCE(wm.created_at, lm.created_at, t.created_at)",
            "ASC",
            ">",
        ),
    }
}

impl TryFrom<TicketRow> for Ticket {
    type Error = StoreError;

    fn try_from(row: TicketRow) -> Result<Self, Self::Error> {
        Ok(Ticket {
            id: TicketId(row.id),
            client_id: ClientId(row.client_id),
            status: parse_status(&row.status)?,
            channel: TicketChannel::parse(&row.channel)?,
            subject: row.subject,
            last_author: parse_author(&row.last_author)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
            closed_at: row.closed_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct KeyedTicketRow {
    #[sqlx(flatten)]
    ticket: TicketRow,
    sort_key: i64,
}

#[derive(sqlx::FromRow)]
struct ViewRow {
    #[sqlx(flatten)]
    ticket: TicketRow,
    sort_at: DateTime<Utc>,
    first_name: Option<String>,
    last_name: Option<String>,
    telegram_username: Option<String>,
    email: Option<String>,
    waiting_since: Option<DateTime<Utc>>,
    last_message_at: Option<DateTime<Utc>>,
    preview: Option<String>,
    preview_author: Option<String>,
    unread: bool,
    ai_state: Option<String>,
    ai_reason: Option<String>,
    ai_handoff_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ClientRow {
    #[sqlx(flatten)]
    ticket: TicketRow,
    preview_author: Option<String>,
    preview_text: Option<String>,
    preview_file: Option<String>,
    preview_at: Option<DateTime<Utc>>,
    unread: i64,
}

impl TryFrom<ClientRow> for ClientTicket {
    type Error = StoreError;

    fn try_from(row: ClientRow) -> Result<Self, Self::Error> {
        let preview = match (row.preview_author, row.preview_at) {
            (Some(author), Some(at)) => Some(TicketPreview {
                author: parse_author(&author)?,
                text: row.preview_text.unwrap_or_default(),
                file: row.preview_file,
                at,
            }),
            _ => None,
        };
        Ok(ClientTicket {
            ticket: row.ticket.try_into()?,
            preview,
            unread: row.unread,
        })
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    ticket_id: i64,
    author: String,
    admin_id: Option<i64>,
    body: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<MessageRow> for TicketMessage {
    type Error = StoreError;

    fn try_from(row: MessageRow) -> Result<Self, Self::Error> {
        Ok(TicketMessage {
            ticket_id: TicketId(row.ticket_id),
            author: parse_author(&row.author)?,
            admin_id: row.admin_id.map(AdminId),
            body: row.body,
            attachments: Vec::new(),
            created_at: row.created_at,
        })
    }
}

fn keyed_page(rows: Vec<KeyedTicketRow>, page: PageRequest) -> StoreResult<Page<Ticket>> {
    let keyed = rows
        .into_iter()
        .map(|row| Ok((row.sort_key, Ticket::try_from(row.ticket)?)))
        .collect::<StoreResult<Vec<(i64, Ticket)>>>()?;
    let page = Page::from_overfetch(keyed, page, |(key, _)| *key);
    Ok(Page {
        items: page.items.into_iter().map(|(_, ticket)| ticket).collect(),
        next: page.next,
    })
}

pub struct TicketsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> TicketsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        TicketsRepo { conn }
    }

    pub async fn open(&mut self, ticket: &NewTicket) -> StoreResult<TicketId> {
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO tickets (client_id, subscription_id, channel, subject, last_author, ai_state) \
             VALUES ($1, $2, $3, $4, 'client', CASE WHEN $5 THEN 'active' END) RETURNING id",
        )
        .bind(ticket.client_id.0)
        .bind(ticket.subscription_id.map(|s| s.0))
        .bind(ticket.channel.as_str())
        .bind(&ticket.subject)
        .bind(ticket.ai)
        .fetch_one(&mut *tx)
        .await?;
        let message: i64 = sqlx::query_scalar(
            "INSERT INTO ticket_messages (ticket_id, author, body) VALUES ($1, 'client', $2) RETURNING id",
        )
        .bind(id)
        .bind(&ticket.body)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE tickets SET last_message_id = $2, waiting_message_id = $2, \
             last_message_at = (SELECT m.created_at FROM ticket_messages m WHERE m.id = $2) WHERE id = $1",
        )
        .bind(id)
        .bind(message)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(TicketId(id))
    }

    pub async fn count_open(&mut self, client_id: ClientId) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE client_id = $1 AND status = 'open'",
        )
        .bind(client_id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn open_in_telegram(
        &mut self,
        client_id: ClientId,
        preferred: Option<TicketId>,
    ) -> StoreResult<Option<TicketId>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM tickets \
             WHERE client_id = $1 AND status = 'open' AND (channel IN ('bot', 'unknown') OR id = $2) \
             ORDER BY id = $2 DESC, updated_at DESC, id DESC LIMIT 1",
        )
        .bind(client_id.0)
        .bind(preferred.map_or(0, |id| id.0))
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(id.map(TicketId))
    }

    pub async fn close(&mut self, id: TicketId) -> StoreResult<()> {
        sqlx::query("UPDATE tickets SET status = 'closed', closed_at = now() WHERE id = $1 AND status = 'open'")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn close_many(&mut self, ids: &[TicketId]) -> StoreResult<Vec<TicketId>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let closed: Vec<i64> = sqlx::query_scalar(
            "UPDATE tickets SET status = 'closed', closed_at = now() \
             WHERE id = ANY($1) AND status = 'open' RETURNING id",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(sorted_ids(closed))
    }

    pub async fn count_waiting_longer(&mut self, days: u32) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT count(*) FROM tickets t WHERE {WAITING_LONGER}"
        ))
        .bind(i32::try_from(days).unwrap_or(i32::MAX))
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn close_waiting_longer(&mut self, days: u32) -> StoreResult<Vec<TicketId>> {
        let closed: Vec<i64> = sqlx::query_scalar(&format!(
            "UPDATE tickets t SET status = 'closed', closed_at = now() WHERE {WAITING_LONGER} RETURNING t.id"
        ))
        .bind(i32::try_from(days).unwrap_or(i32::MAX))
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(sorted_ids(closed))
    }

    pub async fn mark_read(&mut self, id: TicketId) -> StoreResult<u64> {
        Ok(sqlx::query(
            "UPDATE ticket_messages SET read_at = now() WHERE ticket_id = $1 AND author = 'client' AND read_at IS NULL",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?
        .rows_affected())
    }

    pub async fn reopen(&mut self, id: TicketId) -> StoreResult<()> {
        sqlx::query(
            "UPDATE tickets SET status = 'open', closed_at = NULL WHERE id = $1 AND status = 'closed'",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn ai(&mut self, id: TicketId) -> StoreResult<Option<TicketAi>> {
        let row: Option<AiRow> =
            sqlx::query_as("SELECT ai_state, ai_reason, ai_handoff_at FROM tickets WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?;
        let (state, reason, at) = row.ok_or(StoreError::NotFound("обращение"))?;
        ticket_ai(state.as_deref(), reason.as_deref(), at)
    }

    pub async fn take_over(&mut self, id: TicketId) -> StoreResult<bool> {
        Ok(sqlx::query(
            "UPDATE tickets SET ai_state = 'handed_off', ai_reason = 'operator', ai_handoff_at = now(), \
             ai_claimed_until = NULL WHERE id = $1 AND ai_state = 'active'",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn get(&mut self, id: TicketId) -> StoreResult<Ticket> {
        let row: TicketRow =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM tickets t WHERE t.id = $1"))
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?
                .ok_or(StoreError::NotFound("обращение"))?;
        row.try_into()
    }

    pub async fn list_view(
        &mut self,
        filter: TicketFilter,
        order: TicketOrder,
        page: PageRequest,
        after_id: Option<TicketId>,
        query: Option<&str>,
    ) -> StoreResult<Page<TicketListItem>> {
        let condition = filter_condition(filter);
        let (key, direction, compare) = order_parts(order);
        let after = match page.after.map(DateTime::<Utc>::from_timestamp_micros) {
            None => None,
            Some(Some(at)) => Some(at),
            Some(None) => {
                return Ok(Page {
                    items: Vec::new(),
                    next: None,
                });
            }
        };
        let needle = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| q.chars().take(100).collect::<String>());
        let like = needle
            .as_deref()
            .map(|q| format!("%{}%", super::client_search::escape_like(q)));
        let folded = needle.as_deref().map(|q| {
            format!(
                "%{}%",
                super::client_search::escape_like(&super::clients::fold_case(q))
            )
        });
        let rows: Vec<ViewRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, {key} AS sort_at, tg.meta ->> 'first_name' AS first_name, \
             tg.meta ->> 'last_name' AS last_name, tg.meta ->> 'username' AS telegram_username, \
             em.value AS email, \
             CASE WHEN t.status = 'open' AND t.last_author = 'client' \
                 THEN COALESCE(wm.created_at, lm.created_at, t.created_at) END AS waiting_since, \
             lm.created_at AS last_message_at, COALESCE(NULLIF(left(lm.body, {PREVIEW_CHARS}), ''), \
                 (SELECT 'Файл: ' || pf.name FROM ticket_files pf WHERE pf.message_id = lm.id ORDER BY pf.id LIMIT 1)) AS preview, \
             lm.author AS preview_author, \
             EXISTS (SELECT 1 FROM ticket_messages um WHERE um.ticket_id = t.id AND um.author = 'client' \
                 AND um.read_at IS NULL) AS unread, \
             t.ai_state, t.ai_reason, t.ai_handoff_at \
             FROM tickets t \
             LEFT JOIN ticket_messages wm ON wm.id = t.waiting_message_id \
             LEFT JOIN ticket_messages lm ON lm.id = t.last_message_id \
             LEFT JOIN LATERAL (SELECT i.meta FROM client_identities i WHERE i.client_id = t.client_id \
                 AND i.kind = 'telegram' ORDER BY i.id LIMIT 1) tg ON true \
             LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = t.client_id \
                 AND i.kind = 'email' ORDER BY i.id LIMIT 1) em ON true \
             WHERE {condition} AND ($1::timestamptz IS NULL OR ({key}, t.id) {compare} ($1, $5::bigint)) \
             AND ($3::text IS NULL OR t.subject ILIKE $3 \
                 OR EXISTS (SELECT 1 FROM ticket_messages qm WHERE qm.ticket_id = t.id AND qm.body ILIKE $3) \
                 OR EXISTS (SELECT 1 FROM ticket_files qf WHERE qf.ticket_id = t.id AND qf.name ILIKE $3) \
                 OR EXISTS (SELECT 1 FROM client_identities qi WHERE qi.client_id = t.client_id \
                     AND qi.search_text LIKE $4)) \
             ORDER BY {key} {direction}, t.id {direction} LIMIT $2"
        ))
        .bind(after)
        .bind(page.fetch_limit())
        .bind(like)
        .bind(folded)
        .bind(after_id.map(|id| id.0))
        .fetch_all(&mut *self.conn)
        .await?;
        let keyed = rows
            .into_iter()
            .map(|row| {
                Ok((
                    row.sort_at.timestamp_micros(),
                    TicketListItem {
                        ticket: Ticket::try_from(row.ticket)?,
                        client_name: display_name(
                            row.first_name.as_deref(),
                            row.last_name.as_deref(),
                        ),
                        telegram_username: row.telegram_username,
                        email: row.email,
                        waiting_since: row.waiting_since,
                        last_message_at: row.last_message_at,
                        preview: row.preview,
                        last_author: row.preview_author,
                        unread: row.unread,
                        ai: ticket_ai(
                            row.ai_state.as_deref(),
                            row.ai_reason.as_deref(),
                            row.ai_handoff_at,
                        )?,
                    },
                ))
            })
            .collect::<StoreResult<Vec<(i64, TicketListItem)>>>()?;
        let page = Page::from_overfetch(keyed, page, |(key, _)| *key);
        Ok(Page {
            items: page.items.into_iter().map(|(_, item)| item).collect(),
            next: page.next,
        })
    }

    pub async fn for_client(
        &mut self,
        client_id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<Ticket>> {
        let rows: Vec<KeyedTicketRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, t.id AS sort_key FROM tickets t \
             WHERE t.client_id = $1 AND ($2::bigint IS NULL OR t.id < $2) ORDER BY t.id DESC LIMIT $3"
        ))
        .bind(client_id.0)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        keyed_page(rows, page)
    }

    pub async fn client_view(
        &mut self,
        client_id: ClientId,
        limit: i64,
    ) -> StoreResult<Vec<ClientTicket>> {
        let rows: Vec<ClientRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, lm.author AS preview_author, left(lm.body, {PREVIEW_CHARS}) AS preview_text, \
             (SELECT f.name FROM ticket_files f WHERE f.message_id = lm.id ORDER BY f.id LIMIT 1) AS preview_file, \
             lm.created_at AS preview_at, (SELECT count(*) {UNREAD}) AS unread \
             FROM tickets t \
             LEFT JOIN LATERAL (SELECT m.id, m.author, m.body, m.created_at FROM ticket_messages m \
                 WHERE m.ticket_id = t.id AND m.author <> 'system' ORDER BY m.id DESC LIMIT 1) lm ON true \
             WHERE t.client_id = $1 ORDER BY t.status = 'closed', t.updated_at DESC, t.id DESC LIMIT $2"
        ))
        .bind(client_id.0)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(ClientTicket::try_from).collect()
    }

    pub async fn client_unread(&mut self, client_id: ClientId) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT count(*) FROM tickets t WHERE t.client_id = $1 AND EXISTS (SELECT 1 {UNREAD})"
        ))
        .bind(client_id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn mark_seen(
        &mut self,
        id: TicketId,
        client_id: ClientId,
        until: DateTime<Utc>,
    ) -> StoreResult<u64> {
        Ok(sqlx::query(
            "UPDATE ticket_messages m SET read_at = now() FROM tickets t \
             WHERE t.id = $1 AND t.client_id = $2 AND m.ticket_id = t.id \
             AND m.author IN ('support', 'ai') AND m.read_at IS NULL AND m.created_at <= $3",
        )
        .bind(id.0)
        .bind(client_id.0)
        .bind(until)
        .execute(&mut *self.conn)
        .await?
        .rows_affected())
    }

    pub async fn start_unread(&mut self) -> StoreResult<bool> {
        Ok(sqlx::query(
            "INSERT INTO bot_state (key, value) VALUES ($1, to_jsonb(now())) ON CONFLICT (key) DO NOTHING",
        )
        .bind(UNREAD_FROM_KEY)
        .execute(&mut *self.conn)
        .await?
        .rows_affected()
            > 0)
    }

    pub async fn counts(&mut self) -> StoreResult<TicketCounts> {
        let (waiting, answered, closed): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE status = 'open' AND last_author = 'client'), \
             count(*) FILTER (WHERE status = 'open' AND last_author <> 'client'), \
             count(*) FILTER (WHERE status = 'closed') FROM tickets",
        )
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(TicketCounts {
            waiting,
            answered,
            closed,
        })
    }

    pub async fn messages(&mut self, id: TicketId) -> StoreResult<Vec<TicketMessage>> {
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT id, ticket_id, author, admin_id, body, created_at FROM ticket_messages \
             WHERE ticket_id = $1 ORDER BY id",
        )
        .bind(id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        if rows.is_empty() {
            self.get(id).await?;
        }
        let mut files = super::ticket_files::for_ticket(&mut *self.conn, id).await?;
        rows.into_iter()
            .map(|row| {
                let message_id = row.id;
                let mut message = TicketMessage::try_from(row)?;
                message.attachments = files.remove(&message_id).unwrap_or_default();
                Ok(message)
            })
            .collect()
    }

    pub async fn last_message_id(&mut self, id: TicketId) -> StoreResult<i64> {
        let last: Option<Option<i64>> =
            sqlx::query_scalar("SELECT last_message_id FROM tickets WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?;
        last.flatten().ok_or(StoreError::NotFound("обращение"))
    }

    pub async fn delete(&mut self, id: TicketId) -> StoreResult<bool> {
        Ok(sqlx::query("DELETE FROM tickets WHERE id = $1")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?
            .rows_affected()
            > 0)
    }

    pub async fn reply(&mut self, reply: &NewReply) -> StoreResult<()> {
        let mut tx = sqlx::Connection::begin(&mut *self.conn).await?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tickets WHERE id = $1 FOR UPDATE")
                .bind(reply.ticket_id.0)
                .fetch_optional(&mut *tx)
                .await?;
        if found.is_none() {
            return Err(StoreError::NotFound("обращение"));
        }
        let message: i64 = sqlx::query_scalar(
            "INSERT INTO ticket_messages (ticket_id, author, admin_id, body) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(reply.ticket_id.0)
        .bind(author_str(reply.author))
        .bind(reply.admin_id.map(|a| a.0))
        .bind(&reply.body)
        .fetch_one(&mut *tx)
        .await?;
        super::ticket_files::attach(&mut tx, reply.ticket_id, message, &reply.attachments).await?;
        sqlx::query(
            "UPDATE tickets SET last_author = $2, last_message_id = $3, \
             last_message_at = (SELECT m.created_at FROM ticket_messages m WHERE m.id = $3), \
             waiting_message_id = CASE WHEN $2 <> 'client' THEN NULL \
                 WHEN status = 'open' AND last_author = 'client' THEN COALESCE(waiting_message_id, $3) \
                 ELSE $3 END, \
             status = CASE WHEN $4 THEN 'closed' WHEN $2 = 'client' THEN 'open' ELSE status END, \
             closed_at = CASE WHEN $4 THEN COALESCE(closed_at, now()) WHEN $2 = 'client' THEN NULL ELSE closed_at END \
             WHERE id = $1",
        )
        .bind(reply.ticket_id.0)
        .bind(author_str(reply.author))
        .bind(message)
        .bind(reply.close)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
