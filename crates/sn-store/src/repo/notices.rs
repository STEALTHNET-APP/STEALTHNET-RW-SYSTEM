use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::payment::NoticeKind;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPaymentNotice {
    pub provider: String,
    pub event_key: String,
    pub kind: NoticeKind,
    pub payment_id: Option<PaymentId>,
    pub client_id: Option<ClientId>,
    pub amount: Option<Amount>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentNotice {
    pub id: i64,
    pub provider: String,
    pub event_key: String,
    pub kind: NoticeKind,
    pub payment_id: Option<PaymentId>,
    pub client_id: Option<ClientId>,
    pub amount: Option<Amount>,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ClosedCredit {
    pub resolved_at: DateTime<Utc>,
    pub resolution: Option<String>,
}

#[derive(sqlx::FromRow)]
struct NoticeRow {
    id: i64,
    provider: String,
    event_key: String,
    kind: String,
    payment_id: Option<i64>,
    client_id: Option<i64>,
    amount_minor: Option<i64>,
    currency: Option<String>,
    text: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<NoticeRow> for PaymentNotice {
    type Error = crate::error::StoreError;

    fn try_from(row: NoticeRow) -> Result<Self, Self::Error> {
        let amount = match (row.amount_minor, row.currency.as_deref()) {
            (Some(minor), Some(code)) => Some(Amount::new(minor, PayCurrency::parse(code)?)),
            _ => None,
        };
        Ok(PaymentNotice {
            id: row.id,
            provider: row.provider,
            event_key: row.event_key,
            kind: NoticeKind::parse(&row.kind)?,
            payment_id: row.payment_id.map(PaymentId),
            client_id: row.client_id.map(ClientId),
            amount,
            text: row.text,
            created_at: row.created_at,
        })
    }
}

const COLUMNS: &str = "id, provider, event_key, kind, payment_id, client_id, amount_minor, currency, text, created_at";

pub const WAITING_NOTICES: &str =
    "resolved_at IS NULL AND kind IN ('manual_credit', 'refund_reported')";

pub struct PaymentNoticesRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> PaymentNoticesRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        PaymentNoticesRepo { conn }
    }

    pub async fn record(&mut self, notice: &NewPaymentNotice) -> StoreResult<Option<i64>> {
        Ok(sqlx::query_scalar(
            "INSERT INTO payment_notices (provider, event_key, kind, payment_id, client_id, amount_minor, currency, text) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (provider, event_key) DO NOTHING RETURNING id",
        )
        .bind(&notice.provider)
        .bind(&notice.event_key)
        .bind(notice.kind.as_str())
        .bind(notice.payment_id.map(|p| p.0))
        .bind(notice.client_id.map(|c| c.0))
        .bind(notice.amount.map(|a| a.minor))
        .bind(notice.amount.map(|a| a.currency.code()))
        .bind(&notice.text)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn newest_id(&mut self) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar("SELECT coalesce(max(id), 0)::bigint FROM payment_notices")
                .fetch_one(&mut *self.conn)
                .await?,
        )
    }

    pub async fn between(
        &mut self,
        after: i64,
        upto: i64,
        limit: i64,
    ) -> StoreResult<Vec<PaymentNotice>> {
        let rows: Vec<NoticeRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payment_notices WHERE id > $1 AND id <= $2 ORDER BY id LIMIT $3"
        ))
        .bind(after)
        .bind(upto)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn waiting(&mut self, limit: i64) -> StoreResult<Vec<PaymentNotice>> {
        let rows: Vec<NoticeRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payment_notices WHERE {WAITING_NOTICES} ORDER BY created_at, id LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn get(&mut self, id: i64) -> StoreResult<PaymentNotice> {
        let row: Option<NoticeRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payment_notices WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.ok_or(crate::error::StoreError::NotFound("оповещение"))?
            .try_into()
    }

    pub async fn close(
        &mut self,
        id: i64,
        admin: AdminId,
        note: &str,
        at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        let closed: Option<i64> = sqlx::query_scalar(&format!(
            "UPDATE payment_notices SET resolved_at = $2, resolved_by = $3, resolution = $4 \
             WHERE id = $1 AND {WAITING_NOTICES} RETURNING id"
        ))
        .bind(id)
        .bind(at)
        .bind(admin.0)
        .bind(note)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(closed.is_some())
    }

    pub async fn close_credit_event(
        &mut self,
        provider: &str,
        event_key: &str,
        admin: Option<AdminId>,
        note: &str,
        at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        let closed: Option<i64> = sqlx::query_scalar(
            "UPDATE payment_notices SET resolved_at = $3, resolved_by = $4, resolution = $5 \
             WHERE provider = $1 AND event_key = $2 AND kind = 'manual_credit' \
             AND resolved_at IS NULL RETURNING id",
        )
        .bind(provider)
        .bind(event_key)
        .bind(at)
        .bind(admin.map(|a| a.0))
        .bind(note)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(closed.is_some())
    }

    pub async fn closed_credit(
        &mut self,
        provider: &str,
        event_key: &str,
    ) -> StoreResult<Option<ClosedCredit>> {
        Ok(sqlx::query_as(
            "SELECT resolved_at, resolution FROM payment_notices \
             WHERE provider = $1 AND event_key = $2 AND kind = 'manual_credit' \
             AND resolved_at IS NOT NULL",
        )
        .bind(provider)
        .bind(event_key)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn for_payment(&mut self, payment_id: PaymentId) -> StoreResult<Vec<PaymentNotice>> {
        let rows: Vec<NoticeRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payment_notices WHERE payment_id = $1 ORDER BY id"
        ))
        .bind(payment_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }
}
