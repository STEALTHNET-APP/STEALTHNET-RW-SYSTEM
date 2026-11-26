use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::payment::PaymentStatus;
use sqlx::PgConnection;
use uuid::Uuid;

const FINAL: &str = "'accepted', 'duplicate', 'ignored', 'payment_not_found', 'amount_mismatch'";
const TRAIL: &str = "(CASE WHEN jsonb_array_length(webhook_inbox.trail) >= 30 \
     THEN webhook_inbox.trail - 0 ELSE webhook_inbox.trail END)";
const COLUMNS: &str = "w.id, w.source, w.outcome, w.payment_id, p.client_id, w.error, w.remote_ip, \
     w.received_at, w.processed_at, w.claimed_at, w.deliveries, w.parsed, w.event, w.body IS NOT NULL AS has_body, \
     p.public_id AS payment_public_id, p.amount_minor, p.currency, p.status AS payment_status, \
     o.public_id AS order_public_id, w.legacy_path, w.legacy_invoice_id";
const BODY_SEARCH_MIN: usize = 6;
const MASKED_SINCE: &str = "COALESCE((SELECT m.installed_on FROM _sqlx_migrations m WHERE m.version = 290 AND m.success), '-infinity'::timestamptz)";
const PROVIDERS: &str = "WITH RECURSIVE k (v) AS (SELECT min(provider) FROM payments \
     UNION ALL SELECT (SELECT min(m.provider) FROM payments m WHERE m.provider > k.v) FROM k WHERE k.v IS NOT NULL) \
     SELECT v FROM k WHERE v IS NOT NULL";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxOutcome {
    Received,
    Accepted,
    Duplicate,
    Ignored,
    RejectedSignature,
    RejectedPayload,
    RejectedBusiness,
    NotAccepted,
    PaymentNotFound,
    AmountMismatch,
    Error,
}

impl InboxOutcome {
    pub const ALL: [InboxOutcome; 11] = [
        InboxOutcome::Received,
        InboxOutcome::Accepted,
        InboxOutcome::Duplicate,
        InboxOutcome::Ignored,
        InboxOutcome::RejectedSignature,
        InboxOutcome::RejectedPayload,
        InboxOutcome::RejectedBusiness,
        InboxOutcome::NotAccepted,
        InboxOutcome::PaymentNotFound,
        InboxOutcome::AmountMismatch,
        InboxOutcome::Error,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            InboxOutcome::Received => "received",
            InboxOutcome::Accepted => "accepted",
            InboxOutcome::Duplicate => "duplicate",
            InboxOutcome::Ignored => "ignored",
            InboxOutcome::RejectedSignature => "rejected_signature",
            InboxOutcome::RejectedPayload => "rejected_payload",
            InboxOutcome::RejectedBusiness => "rejected_business",
            InboxOutcome::NotAccepted => "not_accepted",
            InboxOutcome::PaymentNotFound => "payment_not_found",
            InboxOutcome::AmountMismatch => "amount_mismatch",
            InboxOutcome::Error => "error",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        Ok(match value {
            "received" => InboxOutcome::Received,
            "accepted" => InboxOutcome::Accepted,
            "duplicate" => InboxOutcome::Duplicate,
            "ignored" => InboxOutcome::Ignored,
            "rejected_signature" => InboxOutcome::RejectedSignature,
            "rejected_payload" => InboxOutcome::RejectedPayload,
            "rejected_business" => InboxOutcome::RejectedBusiness,
            "not_accepted" => InboxOutcome::NotAccepted,
            "payment_not_found" => InboxOutcome::PaymentNotFound,
            "amount_mismatch" => InboxOutcome::AmountMismatch,
            "error" => InboxOutcome::Error,
            other => return Err(StoreError::Decode(format!("исход вебхука: {other}"))),
        })
    }

    pub const fn is_final(self) -> bool {
        matches!(
            self,
            InboxOutcome::Accepted
                | InboxOutcome::Duplicate
                | InboxOutcome::Ignored
                | InboxOutcome::PaymentNotFound
                | InboxOutcome::AmountMismatch
        )
    }

    pub const fn is_unverified(self) -> bool {
        matches!(
            self,
            InboxOutcome::RejectedSignature
                | InboxOutcome::RejectedPayload
                | InboxOutcome::NotAccepted
                | InboxOutcome::Error
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInboundWebhook {
    pub source: String,
    pub dedupe_key: String,
    pub remote_ip: Option<String>,
    pub headers: serde_json::Value,
    pub body: Option<Vec<u8>>,
    pub parsed: Option<serde_json::Value>,
    pub event: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxRecord {
    pub id: i64,
    pub duplicate: bool,
    pub outcome: InboxOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEntry {
    pub id: i64,
    pub source: String,
    pub outcome: InboxOutcome,
    pub has_body: bool,
    pub payment_id: Option<PaymentId>,
    pub error: Option<String>,
    pub remote_ip: Option<String>,
    pub received_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct InboxEntryRow {
    id: i64,
    source: String,
    outcome: String,
    has_body: bool,
    payment_id: Option<i64>,
    error: Option<String>,
    remote_ip: Option<String>,
    received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetryClaim {
    pub source: String,
    pub dedupe_key: String,
    pub parsed: serde_json::Value,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalFilter {
    pub source: Option<String>,
    pub outcomes: Vec<InboxOutcome>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPayment {
    pub public_id: Uuid,
    pub amount: Amount,
    pub status: PaymentStatus,
    pub order_public_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalRow {
    pub id: i64,
    pub source: String,
    pub outcome: InboxOutcome,
    pub payment_id: Option<PaymentId>,
    pub client_id: Option<ClientId>,
    pub error: Option<String>,
    pub remote_ip: Option<String>,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub deliveries: i32,
    pub parsed: Option<serde_json::Value>,
    pub event: Option<String>,
    pub has_body: bool,
    pub payment: Option<JournalPayment>,
    pub legacy_path: bool,
    pub legacy_invoice: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrailKind {
    Delivery,
    Processed,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrailStep {
    pub at: DateTime<Utc>,
    pub step: TrailKind,
    #[serde(default)]
    pub outcome: Option<InboxOutcome>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub admin_id: Option<AdminId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    pub row: JournalRow,
    pub headers: serde_json::Value,
    pub body: Option<Vec<u8>>,
    pub trail: Vec<TrailStep>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InboxAttention {
    pub rejected: i64,
    pub failed: i64,
    pub sources: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct JournalRowData {
    id: i64,
    source: String,
    outcome: String,
    payment_id: Option<i64>,
    client_id: Option<i64>,
    error: Option<String>,
    remote_ip: Option<String>,
    received_at: DateTime<Utc>,
    processed_at: Option<DateTime<Utc>>,
    claimed_at: Option<DateTime<Utc>>,
    deliveries: i32,
    parsed: Option<serde_json::Value>,
    event: Option<String>,
    has_body: bool,
    payment_public_id: Option<Uuid>,
    amount_minor: Option<i64>,
    currency: Option<String>,
    payment_status: Option<String>,
    order_public_id: Option<Uuid>,
    legacy_path: bool,
    legacy_invoice_id: Option<i64>,
}

impl JournalRowData {
    fn into_row(self) -> StoreResult<JournalRow> {
        let payment = match (
            self.payment_public_id,
            self.amount_minor,
            self.currency.as_deref(),
            self.payment_status.as_deref(),
        ) {
            (Some(public_id), Some(minor), Some(currency), Some(status)) => Some(JournalPayment {
                public_id,
                amount: Amount::new(minor, PayCurrency::parse(currency)?),
                status: PaymentStatus::parse(status)?,
                order_public_id: self.order_public_id,
            }),
            _ => None,
        };
        Ok(JournalRow {
            id: self.id,
            source: self.source,
            outcome: InboxOutcome::parse(&self.outcome)?,
            payment_id: self.payment_id.map(PaymentId),
            client_id: self.client_id.map(ClientId),
            error: self.error,
            remote_ip: self.remote_ip,
            received_at: self.received_at,
            processed_at: self.processed_at,
            claimed_at: self.claimed_at,
            deliveries: self.deliveries,
            parsed: self.parsed,
            event: self.event,
            has_body: self.has_body,
            payment,
            legacy_path: self.legacy_path,
            legacy_invoice: self.legacy_invoice_id.is_some(),
        })
    }
}

#[derive(sqlx::FromRow)]
struct JournalEntryData {
    #[sqlx(flatten)]
    row: JournalRowData,
    headers: serde_json::Value,
    body: Option<Vec<u8>>,
    trail: serde_json::Value,
}

pub struct WebhooksRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> WebhooksRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        WebhooksRepo { conn }
    }

    pub async fn record(&mut self, hook: &NewInboundWebhook) -> StoreResult<InboxRecord> {
        let (id, outcome, deliveries): (i64, String, i32) = sqlx::query_as(&format!(
            "INSERT INTO webhook_inbox (source, dedupe_key, remote_ip, headers, body, parsed, event) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (source, dedupe_key) DO UPDATE SET deliveries = webhook_inbox.deliveries + 1, \
             event = COALESCE(webhook_inbox.event, EXCLUDED.event), \
             parsed = CASE WHEN webhook_inbox.outcome IN ({FINAL}) OR EXCLUDED.parsed IS NULL \
             THEN webhook_inbox.parsed ELSE EXCLUDED.parsed END, \
             trail = {TRAIL} || jsonb_build_array(jsonb_build_object('at', now(), 'step', 'delivery')) \
             RETURNING id, outcome, deliveries"
        ))
        .bind(&hook.source)
        .bind(&hook.dedupe_key)
        .bind(&hook.remote_ip)
        .bind(&hook.headers)
        .bind(&hook.body)
        .bind(&hook.parsed)
        .bind(&hook.event)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(InboxRecord {
            id,
            duplicate: deliveries > 1,
            outcome: InboxOutcome::parse(&outcome)?,
        })
    }

    pub async fn mark_legacy(
        &mut self,
        source: &str,
        dedupe_key: &str,
        path: bool,
        invoice: Option<i64>,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE webhook_inbox SET legacy_path = legacy_path OR $3, \
             legacy_invoice_id = COALESCE($4, legacy_invoice_id) WHERE source = $1 AND dedupe_key = $2",
        )
        .bind(source)
        .bind(dedupe_key)
        .bind(path)
        .bind(invoice)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn finish(
        &mut self,
        id: i64,
        outcome: InboxOutcome,
        payment_id: Option<PaymentId>,
        error: Option<&str>,
        admin: Option<AdminId>,
    ) -> StoreResult<()> {
        sqlx::query(&format!(
            "UPDATE webhook_inbox SET \
             outcome = CASE WHEN outcome = 'accepted' THEN outcome ELSE $2 END, \
             payment_id = COALESCE($3, payment_id), \
             error = CASE WHEN outcome = 'accepted' THEN error ELSE $4 END, \
             processed_at = now(), claimed_at = NULL, \
             trail = {TRAIL} || jsonb_build_array(jsonb_strip_nulls(jsonb_build_object(\
             'at', now(), 'step', 'processed', 'outcome', $2::text, 'note', $4::text, 'admin_id', $5::bigint))) \
             WHERE id = $1"
        ))
        .bind(id)
        .bind(outcome.as_str())
        .bind(payment_id.map(|p| p.0))
        .bind(error)
        .bind(admin.map(|a| a.0))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn record_rejected(
        &mut self,
        hook: &NewInboundWebhook,
        outcome: InboxOutcome,
        error: Option<&str>,
    ) -> StoreResult<bool> {
        if !outcome.is_unverified() {
            return Err(StoreError::Invalid(format!(
                "отказ вебхука не может иметь исход {}",
                outcome.as_str()
            )));
        }
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO webhook_inbox (source, dedupe_key, remote_ip, headers, body, outcome, error, processed_at, event, trail) \
             VALUES ($1, $2, $3, $4, NULL, $5, $6, now(), $7, jsonb_build_array(jsonb_strip_nulls(jsonb_build_object(\
             'at', now(), 'step', 'processed', 'outcome', $5::text, 'note', $6::text)))) \
             ON CONFLICT (source, dedupe_key) DO NOTHING RETURNING id",
        )
        .bind(&hook.source)
        .bind(&hook.dedupe_key)
        .bind(&hook.remote_ip)
        .bind(&hook.headers)
        .bind(outcome.as_str())
        .bind(error)
        .bind(&hook.event)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(inserted.is_some())
    }

    pub async fn trace(
        &mut self,
        source: &str,
        since: DateTime<Utc>,
    ) -> StoreResult<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
        Ok(sqlx::query_as(
            "SELECT \
                 max(received_at) FILTER (WHERE outcome IN ('accepted', 'duplicate', 'ignored', 'payment_not_found', 'amount_mismatch', 'rejected_business')), \
                 max(received_at) FILTER (WHERE outcome = 'rejected_signature') \
             FROM webhook_inbox WHERE source = $1 AND received_at >= $2",
        )
        .bind(source)
        .bind(since)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn claim_retry(
        &mut self,
        id: i64,
        admin: AdminId,
        stale_before: DateTime<Utc>,
        ignored: &str,
    ) -> StoreResult<Option<RetryClaim>> {
        type Claimed = (String, String, Option<serde_json::Value>, Option<Vec<u8>>);
        let row: Option<Claimed> = sqlx::query_as(&format!(
            "UPDATE webhook_inbox SET outcome = 'received', claimed_at = now(), \
             trail = {TRAIL} || jsonb_build_array(jsonb_build_object('at', now(), 'step', 'retry', 'admin_id', $2::bigint)) \
             WHERE id = $1 AND parsed IS NOT NULL AND (outcome = 'error' OR \
             (outcome = 'ignored' AND error = $4) OR \
             (outcome = 'received' AND COALESCE(claimed_at, received_at) < $3)) \
             RETURNING source, dedupe_key, parsed, body"
        ))
        .bind(id)
        .bind(admin.0)
        .bind(stale_before)
        .bind(ignored)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.and_then(|(source, dedupe_key, parsed, body)| {
            parsed.map(|parsed| RetryClaim {
                source,
                dedupe_key,
                parsed,
                body,
            })
        }))
    }

    pub async fn claim(&mut self, id: i64, stale_before: DateTime<Utc>) -> StoreResult<bool> {
        let claimed: Option<i64> = sqlx::query_scalar(
            "UPDATE webhook_inbox SET outcome = 'received', claimed_at = now() \
             WHERE id = $1 AND (outcome IN ('error', 'rejected_business') OR \
             (outcome = 'received' AND COALESCE(claimed_at, received_at) < $2)) \
             RETURNING id",
        )
        .bind(id)
        .bind(stale_before)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(claimed.is_some())
    }

    pub async fn reversal(
        &mut self,
        id: i64,
        source: &str,
        payment_id: PaymentId,
        txid: Option<&str>,
    ) -> StoreResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT parsed->'status'->>'type' FROM webhook_inbox \
             WHERE source = $1 AND payment_id = $2 AND id <> $3 AND parsed IS NOT NULL \
             AND (parsed->'status'->>'type' IN ('refunded', 'partially_refunded') \
             OR (id > $3 AND parsed->'status'->>'type' IN ('canceled', 'failed') \
             AND ($4::text IS NULL OR parsed->>'provider_txid' IS NULL OR parsed->>'provider_txid' = $4))) \
             ORDER BY id LIMIT 1",
        )
        .bind(source)
        .bind(payment_id.0)
        .bind(id)
        .bind(txid)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn journal(
        &mut self,
        filter: &JournalFilter,
        after: Option<i64>,
        limit: i64,
    ) -> StoreResult<Vec<JournalRow>> {
        let search = filter
            .search
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let number = search.and_then(|q| q.parse::<i64>().ok());
        let public = search.and_then(|q| Uuid::parse_str(q).ok());
        let wide = search.filter(|q| q.chars().count() >= BODY_SEARCH_MIN);
        let outcomes: Option<Vec<&str>> = (!filter.outcomes.is_empty())
            .then(|| filter.outcomes.iter().map(|o| o.as_str()).collect());
        let body = if wide.is_some() {
            format!(
                " OR (w.body IS NOT NULL AND w.received_at >= {MASKED_SINCE} \
                 AND position(convert_to($10, 'UTF8') in w.body) > 0 \
                 AND encode(w.body, 'escape') LIKE '%' || replace(replace(replace(\
                 encode(convert_to($10, 'UTF8'), 'escape'), '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '%')"
            )
        } else {
            String::new()
        };
        let found = if search.is_some() {
            format!(
                "AND (w.payment_id = ANY(ARRAY(SELECT $7 WHERE $7 IS NOT NULL \
                 UNION ALL SELECT q.id FROM payments q WHERE q.public_id = $8 \
                 UNION ALL SELECT q.id FROM payments q JOIN orders r ON r.id = q.order_id WHERE r.public_id = $8 \
                 UNION ALL SELECT q.id FROM payments q WHERE q.provider_txid = $6 AND q.provider IN ({PROVIDERS}))) \
                 OR w.parsed->>'provider_txid' = $6 OR w.parsed->>'event_key' = $6 \
                 OR w.parsed->'payment'->>'value' = $6{body}) "
            )
        } else {
            String::new()
        };
        let rows: Vec<JournalRowData> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM webhook_inbox w \
             LEFT JOIN payments p ON p.id = w.payment_id \
             LEFT JOIN orders o ON o.id = p.order_id \
             WHERE ($1::text IS NULL OR w.source = $1) \
             AND ($2::text[] IS NULL OR w.outcome = ANY($2)) \
             AND ($3::timestamptz IS NULL OR w.received_at >= $3) \
             AND ($4::timestamptz IS NULL OR w.received_at < $4) \
             AND ($5::bigint IS NULL OR w.id < $5) \
             {found}ORDER BY w.id DESC LIMIT $9"
        ))
        .persistent(search.is_none())
        .bind(filter.source.as_deref())
        .bind(outcomes)
        .bind(filter.since)
        .bind(filter.until)
        .bind(after)
        .bind(search)
        .bind(number)
        .bind(public)
        .bind(limit)
        .bind(wide)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(JournalRowData::into_row).collect()
    }

    pub async fn journal_entry(&mut self, id: i64) -> StoreResult<Option<JournalEntry>> {
        let row: Option<JournalEntryData> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, w.headers, w.body, w.trail FROM webhook_inbox w \
             LEFT JOIN payments p ON p.id = w.payment_id \
             LEFT JOIN orders o ON o.id = p.order_id \
             WHERE w.id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(|data| {
            Ok(JournalEntry {
                row: data.row.into_row()?,
                headers: data.headers,
                body: data.body,
                trail: serde_json::from_value(data.trail).unwrap_or_default(),
            })
        })
        .transpose()
    }

    pub async fn attention(
        &mut self,
        since: DateTime<Utc>,
        stale_before: DateTime<Utc>,
    ) -> StoreResult<InboxAttention> {
        let (rejected, failed, sources): (i64, i64, Vec<String>) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE outcome IN ('rejected_signature', 'rejected_payload', \
             'rejected_business', 'not_accepted', 'amount_mismatch')), \
             count(*) FILTER (WHERE outcome = 'error' OR (outcome = 'received' AND \
             COALESCE(claimed_at, received_at) < $2)), \
             COALESCE(array_agg(DISTINCT source) FILTER (WHERE outcome <> 'received' OR \
             COALESCE(claimed_at, received_at) < $2), '{}') \
             FROM webhook_inbox WHERE received_at >= $1 AND outcome IN ('received', 'rejected_signature', \
             'rejected_payload', 'rejected_business', 'not_accepted', 'amount_mismatch', 'error')",
        )
        .bind(since)
        .bind(stale_before)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(InboxAttention {
            rejected,
            failed,
            sources,
        })
    }

    pub async fn recent(&mut self, limit: i64) -> StoreResult<Vec<InboxEntry>> {
        let rows: Vec<InboxEntryRow> = sqlx::query_as(
            "SELECT id, source, outcome, body IS NOT NULL AS has_body, payment_id, error, remote_ip, received_at \
             FROM webhook_inbox ORDER BY id DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(InboxEntry {
                    id: row.id,
                    source: row.source,
                    outcome: InboxOutcome::parse(&row.outcome)?,
                    has_body: row.has_body,
                    payment_id: row.payment_id.map(PaymentId),
                    error: row.error,
                    remote_ip: row.remote_ip,
                    received_at: row.received_at,
                })
            })
            .collect()
    }

    pub async fn purge(
        &mut self,
        kept_since: DateTime<Utc>,
        rejected_kept_since: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<u64> {
        let rejected = sqlx::query(
            "DELETE FROM webhook_inbox WHERE id IN (SELECT id FROM webhook_inbox \
             WHERE (outcome = 'rejected_signature' OR body IS NULL) AND received_at < $1 \
             ORDER BY received_at LIMIT $2)",
        )
        .bind(rejected_kept_since)
        .bind(limit)
        .execute(&mut *self.conn)
        .await?
        .rows_affected();
        let expired = sqlx::query(
            "DELETE FROM webhook_inbox WHERE id IN (SELECT id FROM webhook_inbox \
             WHERE received_at < $1 ORDER BY received_at LIMIT $2)",
        )
        .bind(kept_since)
        .bind(limit)
        .execute(&mut *self.conn)
        .await?
        .rows_affected();
        Ok(rejected + expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::testing::test_store;
    use chrono::Duration;

    const WAITS: &str = "ждёт договора";

    fn hook(key: &str, body: Option<&[u8]>) -> NewInboundWebhook {
        NewInboundWebhook {
            source: "cryptopay".into(),
            dedupe_key: key.into(),
            remote_ip: Some("203.0.113.7".into()),
            headers: serde_json::json!({}),
            body: body.map(<[u8]>::to_vec),
            parsed: None,
            event: None,
        }
    }

    async fn keys(db: &mut impl Db) -> Vec<String> {
        sqlx::query_scalar("SELECT dedupe_key FROM webhook_inbox ORDER BY id")
            .fetch_all(db.conn())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn отказ_записывается_без_тела_и_один_раз() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let rejected = hook("rejected:aa", None);
        assert!(
            conn.webhooks()
                .record_rejected(&rejected, InboxOutcome::RejectedSignature, Some("подпись"))
                .await
                .unwrap()
        );
        assert!(
            !conn
                .webhooks()
                .record_rejected(&rejected, InboxOutcome::RejectedSignature, Some("подпись"))
                .await
                .unwrap()
        );
        assert!(
            conn.webhooks()
                .record_rejected(&hook("x", None), InboxOutcome::Accepted, None)
                .await
                .is_err()
        );
        let again = conn.webhooks().record(&rejected).await.unwrap();
        assert!(again.duplicate);
        assert_eq!(again.outcome, InboxOutcome::RejectedSignature);
        let journal = conn.webhooks().recent(10).await.unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].source, "cryptopay");
        assert_eq!(journal[0].outcome, InboxOutcome::RejectedSignature);
        assert!(!journal[0].has_body);
        assert_eq!(journal[0].payment_id, None);
        assert_eq!(journal[0].error.as_deref(), Some("подпись"));
        assert_eq!(journal[0].remote_ip.as_deref(), Some("203.0.113.7"));
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT body FROM webhook_inbox WHERE dedupe_key = 'rejected:aa'")
                .fetch_one(conn.conn())
                .await
                .unwrap();
        assert_eq!(stored, None);
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn засчитанное_уведомление_не_теряет_итог_а_ход_обработки_копится() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let mut first = hook("paid:1", Some(b"{}"));
        first.parsed = Some(serde_json::json!({ "status": { "type": "paid" } }));
        first.event = Some("invoice_paid".into());
        let record = conn.webhooks().record(&first).await.unwrap();
        assert!(!record.duplicate);
        conn.webhooks()
            .finish(record.id, InboxOutcome::Accepted, None, None, None)
            .await
            .unwrap();
        conn.webhooks()
            .finish(record.id, InboxOutcome::Error, None, Some("сбой"), None)
            .await
            .unwrap();
        conn.webhooks()
            .finish(record.id, InboxOutcome::Duplicate, None, None, None)
            .await
            .unwrap();
        for _ in 0..40 {
            let again = conn.webhooks().record(&first).await.unwrap();
            assert!(again.duplicate);
            assert_eq!(again.outcome, InboxOutcome::Accepted);
        }
        let entry = conn
            .webhooks()
            .journal_entry(record.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.row.outcome, InboxOutcome::Accepted);
        assert_eq!(entry.row.error, None);
        assert_eq!(entry.row.deliveries, 41);
        assert_eq!(entry.row.event.as_deref(), Some("invoice_paid"));
        assert_eq!(entry.trail.len(), 30);
        assert!(
            entry
                .trail
                .iter()
                .all(|step| step.step == TrailKind::Delivery)
        );

        let second = conn
            .webhooks()
            .record(&hook("paid:2", Some(b"{}")))
            .await
            .unwrap();
        conn.webhooks()
            .finish(second.id, InboxOutcome::Error, None, Some("сбой"), None)
            .await
            .unwrap();
        assert!(
            conn.webhooks()
                .claim_retry(second.id, AdminId(1), Utc::now(), WAITS)
                .await
                .unwrap()
                .is_none()
        );
        let mut third = hook("paid:3", Some(b"{}"));
        third.parsed = Some(serde_json::json!({ "status": { "type": "paid" } }));
        let third = conn.webhooks().record(&third).await.unwrap();
        conn.webhooks()
            .finish(third.id, InboxOutcome::Error, None, Some("сбой"), None)
            .await
            .unwrap();
        let claim = conn
            .webhooks()
            .claim_retry(third.id, AdminId(1), Utc::now(), WAITS)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.source, "cryptopay");
        assert_eq!(claim.dedupe_key, "paid:3");
        assert_eq!(claim.body.as_deref(), Some(&b"{}"[..]));
        assert!(
            conn.webhooks()
                .claim_retry(
                    third.id,
                    AdminId(1),
                    Utc::now() - Duration::minutes(10),
                    WAITS
                )
                .await
                .unwrap()
                .is_none()
        );
        conn.webhooks()
            .finish(
                third.id,
                InboxOutcome::Duplicate,
                None,
                None,
                Some(AdminId(1)),
            )
            .await
            .unwrap();
        let entry = conn
            .webhooks()
            .journal_entry(third.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.row.outcome, InboxOutcome::Duplicate);
        assert_eq!(entry.row.claimed_at, None);
        let kinds: Vec<(TrailKind, Option<InboxOutcome>, Option<AdminId>)> = entry
            .trail
            .iter()
            .map(|step| (step.step, step.outcome, step.admin_id))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (TrailKind::Processed, Some(InboxOutcome::Error), None),
                (TrailKind::Retry, None, Some(AdminId(1))),
                (
                    TrailKind::Processed,
                    Some(InboxOutcome::Duplicate),
                    Some(AdminId(1))
                ),
            ]
        );
        for (key, reason, claimed) in [("paid:4", WAITS, true), ("paid:5", "не наше", false)]
        {
            let mut hook = hook(key, Some(b"{\"contractId\":\"c-4\"}"));
            hook.parsed = Some(serde_json::json!({ "status": { "type": "paid" } }));
            let record = conn.webhooks().record(&hook).await.unwrap();
            conn.webhooks()
                .finish(record.id, InboxOutcome::Ignored, None, Some(reason), None)
                .await
                .unwrap();
            let claim = conn
                .webhooks()
                .claim_retry(record.id, AdminId(1), Utc::now(), WAITS)
                .await
                .unwrap();
            assert_eq!(claim.is_some(), claimed, "{key}");
            if let Some(claim) = claim {
                assert_eq!(
                    claim.body.as_deref(),
                    Some(&b"{\"contractId\":\"c-4\"}"[..])
                );
            }
        }
        drop(conn);
        db.cleanup().await;
    }

    async fn found(db: &mut impl Db, q: &str) -> Vec<i64> {
        let filter = JournalFilter {
            search: Some(q.into()),
            ..JournalFilter::default()
        };
        db.webhooks()
            .journal(&filter, None, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect()
    }

    #[tokio::test]
    async fn поиск_по_телу_ищет_строку_буквально() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let bodies: [(&str, &[u8]); 4] = [
            ("ru", "{\"description\":\"Оплата заказа №5\"}".as_bytes()),
            ("signs", br#"{"note":"50%_off\path"}"#),
            ("raw", b"\xc0\xff\x00marker-123456\x80"),
            ("plain", b"{\"id\":\"pay-777777\"}"),
        ];
        let mut ids = Vec::new();
        for (key, body) in bodies {
            ids.push(
                conn.webhooks()
                    .record(&hook(key, Some(body)))
                    .await
                    .unwrap()
                    .id,
            );
        }
        let [ru, signs, raw, plain] = ids[..] else {
            panic!("ожидалось четыре уведомления");
        };
        let mut fields = hook("fields", None);
        fields.parsed = Some(serde_json::json!({
            "provider_txid": "tx-1",
            "event_key": "ev:1",
            "payment": { "type": "public_id", "value": "v-9" }
        }));
        let with_fields = conn.webhooks().record(&fields).await.unwrap().id;

        assert_eq!(found(&mut conn, "Оплата заказа").await, vec![ru]);
        assert_eq!(found(&mut conn, "заказа №5").await, vec![ru]);
        assert!(found(&mut conn, "оплата заказа").await.is_empty());
        assert_eq!(found(&mut conn, "50%_off\\path").await, vec![signs]);
        assert!(found(&mut conn, "50%off").await.is_empty());
        assert!(found(&mut conn, "5_%_off").await.is_empty());
        assert!(found(&mut conn, "off\\\\path").await.is_empty());
        assert_eq!(found(&mut conn, "marker-123456").await, vec![raw]);
        assert_eq!(found(&mut conn, "pay-777777").await, vec![plain]);
        assert_eq!(found(&mut conn, "\"pay-777").await, vec![plain]);
        assert!(found(&mut conn, "y-777").await.is_empty());
        assert_eq!(found(&mut conn, "tx-1").await, vec![with_fields]);
        assert_eq!(found(&mut conn, "ev:1").await, vec![with_fields]);
        assert_eq!(found(&mut conn, " v-9 ").await, vec![with_fields]);
        assert!(found(&mut conn, "tx-").await.is_empty());
        assert!(found(&mut conn, "ничего такого").await.is_empty());
        drop(conn);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn старые_записи_удаляются_пачками_по_сроку() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let now = Utc::now();
        for (key, outcome, body, days) in [
            ("a-old", "accepted", Some(b"{}".as_slice()), 40),
            ("a-new", "accepted", Some(b"{}".as_slice()), 10),
            ("r-old", "rejected_signature", Some(b"junk".as_slice()), 4),
            ("r-new", "rejected_payload", None, 1),
            ("e-old", "error", None, 5),
            ("e-body", "error", Some(b"{}".as_slice()), 5),
        ] {
            sqlx::query(
                "INSERT INTO webhook_inbox (source, dedupe_key, outcome, body, received_at) \
                 VALUES ('cryptopay', $1, $2, $3, $4)",
            )
            .bind(key)
            .bind(outcome)
            .bind(body)
            .bind(now - Duration::days(days))
            .execute(conn.conn())
            .await
            .unwrap();
        }
        let purged = conn
            .webhooks()
            .purge(now - Duration::days(30), now - Duration::days(3), 1)
            .await
            .unwrap();
        assert_eq!(purged, 2);
        let purged = conn
            .webhooks()
            .purge(now - Duration::days(30), now - Duration::days(3), 100)
            .await
            .unwrap();
        assert_eq!(purged, 1);
        assert_eq!(keys(&mut conn).await, ["a-new", "r-new", "e-body"]);
        drop(conn);
        db.cleanup().await;
    }
    #[tokio::test]
    async fn подлинные_отклонённые_уведомления_хранятся_дольше_отказов_по_подписи() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let now = Utc::now();
        for (key, outcome, body, days) in [
            ("b-10", "rejected_business", Some(b"{}".as_slice()), 10),
            ("b-31", "rejected_business", Some(b"{}".as_slice()), 31),
            ("p-5", "rejected_payload", Some(b"{}".as_slice()), 5),
            ("s-4", "rejected_signature", Some(b"junk".as_slice()), 4),
            ("n-4", "not_accepted", None, 4),
            ("n-1", "not_accepted", None, 1),
        ] {
            sqlx::query(
                "INSERT INTO webhook_inbox (source, dedupe_key, outcome, body, received_at) \
                 VALUES ('cryptopay', $1, $2, $3, $4)",
            )
            .bind(key)
            .bind(outcome)
            .bind(body)
            .bind(now - Duration::days(days))
            .execute(conn.conn())
            .await
            .unwrap();
        }
        let purged = conn
            .webhooks()
            .purge(now - Duration::days(30), now - Duration::days(3), 100)
            .await
            .unwrap();
        assert_eq!(purged, 3);
        assert_eq!(keys(&mut conn).await, ["b-10", "p-5", "n-1"]);
        assert!(
            conn.webhooks()
                .record_rejected(&hook("y", None), InboxOutcome::RejectedBusiness, None)
                .await
                .is_err()
        );
        drop(conn);
        db.cleanup().await;
    }
}
