use super::subscriptions::PAID_LIKE;
use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sn_domain::ids::{AdminId, ClientId};
use sn_domain::money::{Currency, Money};
use sn_domain::paging::{Page, PageRequest};
use sqlx::PgConnection;

pub mod referral_audit {
    pub const REFERRER_SET: &str = "client.referrer_set";
    pub const REFERRER_REMOVED: &str = "client.referrer_removed";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferralLink {
    pub referrer_id: ClientId,
    pub referrer_blocked: bool,
    pub referrer_deleted: bool,
    pub linked_at: DateTime<Utc>,
    pub set_by: Option<AdminId>,
    pub set_by_email: Option<String>,
    pub set_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    referrer_id: i64,
    referrer_blocked: bool,
    referrer_deleted: bool,
    linked_at: DateTime<Utc>,
    set_by: Option<i64>,
    set_by_email: Option<String>,
    set_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferralOrigin {
    pub source: Option<String>,
    pub start_param: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitedClient {
    pub client_id: ClientId,
    pub joined_at: DateTime<Utc>,
    pub linked_at: DateTime<Utc>,
    pub set_at: Option<DateTime<Utc>>,
    pub blocked: bool,
    pub deleted: bool,
    pub paid: bool,
    pub bonus: Money,
}

#[derive(sqlx::FromRow)]
struct InvitedRow {
    client_id: i64,
    joined_at: DateTime<Utc>,
    linked_at: DateTime<Utc>,
    set_at: Option<DateTime<Utc>>,
    blocked: bool,
    deleted: bool,
    paid: bool,
    bonus_minor: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferralTotals {
    pub invited: i64,
    pub invited_paid: i64,
    pub earned: Money,
    pub withdrawn: Money,
    pub pending: Money,
}

#[derive(sqlx::FromRow)]
struct TotalsRow {
    invited: i64,
    invited_paid: i64,
    earned: i64,
    withdrawn: i64,
    pending: i64,
}

pub struct ReferralsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> ReferralsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        ReferralsRepo { conn }
    }

    pub async fn link(&mut self, client: ClientId) -> StoreResult<Option<ReferralLink>> {
        let row: Option<LinkRow> = sqlx::query_as(
            "SELECT l.referrer_id, r.status = 'blocked' AS referrer_blocked, \
             r.deleted_at IS NOT NULL AS referrer_deleted, l.created_at AS linked_at, \
             l.set_by, a.email AS set_by_email, l.set_at \
             FROM referral_links l JOIN clients r ON r.id = l.referrer_id \
             LEFT JOIN admins a ON a.id = l.set_by WHERE l.client_id = $1",
        )
        .bind(client.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.map(|row| ReferralLink {
            referrer_id: ClientId(row.referrer_id),
            referrer_blocked: row.referrer_blocked,
            referrer_deleted: row.referrer_deleted,
            linked_at: row.linked_at,
            set_by: row.set_by.map(AdminId),
            set_by_email: row.set_by_email,
            set_at: row.set_at,
        }))
    }

    pub async fn lock_referrer(&mut self, client: ClientId) -> StoreResult<Option<ClientId>> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT referrer_id FROM referral_links WHERE client_id = $1 FOR UPDATE",
        )
        .bind(client.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(id.map(ClientId))
    }

    pub async fn origin(&mut self, client: ClientId) -> StoreResult<ReferralOrigin> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT payload ->> 'source', payload ->> 'start_param' FROM domain_events \
             WHERE client_id = $1 AND kind = 'client_registered' ORDER BY id LIMIT 1",
        )
        .bind(client.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row
            .map(|(source, start_param)| ReferralOrigin {
                source,
                start_param,
            })
            .unwrap_or_default())
    }

    pub async fn invited(
        &mut self,
        referrer: ClientId,
        currency: Currency,
        page: PageRequest,
    ) -> StoreResult<Page<InvitedClient>> {
        let offset = page.after.unwrap_or(0).max(0);
        let rows: Vec<InvitedRow> = sqlx::query_as(&format!(
            "SELECT l.client_id, c.created_at AS joined_at, l.created_at AS linked_at, l.set_at, \
             c.status = 'blocked' AS blocked, c.deleted_at IS NOT NULL AS deleted, \
             EXISTS (SELECT 1 FROM payments p WHERE p.client_id = l.client_id \
                 AND p.status IN {PAID_LIKE} AND p.provider <> 'balance') AS paid, \
             COALESCE((SELECT sum(r.amount_minor - r.reversed_minor) FROM referral_credits r \
                 WHERE r.referrer_id = l.referrer_id AND r.referral_id = l.client_id \
                 AND r.currency = $2 AND r.credited_at IS NOT NULL), 0)::bigint AS bonus_minor \
             FROM referral_links l JOIN clients c ON c.id = l.client_id \
             WHERE l.referrer_id = $1 \
             ORDER BY c.created_at DESC, l.client_id DESC OFFSET $3 LIMIT $4"
        ))
        .bind(referrer.0)
        .bind(currency.code())
        .bind(offset)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let more = rows.len() > page.limit as usize;
        let items = rows
            .into_iter()
            .map(|row| InvitedClient {
                client_id: ClientId(row.client_id),
                joined_at: row.joined_at,
                linked_at: row.linked_at,
                set_at: row.set_at,
                blocked: row.blocked,
                deleted: row.deleted,
                paid: row.paid,
                bonus: Money::new(row.bonus_minor, currency),
            })
            .take(page.limit as usize)
            .collect();
        Ok(Page {
            items,
            next: more.then(|| offset + i64::from(page.limit)),
        })
    }

    pub async fn totals(
        &mut self,
        referrer: ClientId,
        currency: Currency,
    ) -> StoreResult<ReferralTotals> {
        let row: TotalsRow = sqlx::query_as(&format!(
            "SELECT \
             (SELECT count(*) FROM referral_links l WHERE l.referrer_id = $1) AS invited, \
             (SELECT count(*) FROM referral_links l WHERE l.referrer_id = $1 \
                 AND EXISTS (SELECT 1 FROM payments p WHERE p.client_id = l.client_id \
                 AND p.status IN {PAID_LIKE} AND p.provider <> 'balance')) AS invited_paid, \
             COALESCE((SELECT sum(r.amount_minor - r.reversed_minor) FROM referral_credits r \
                 WHERE r.referrer_id = $1 AND r.currency = $2 AND r.credited_at IS NOT NULL), 0)::bigint AS earned, \
             COALESCE((SELECT sum(w.amount_minor) FROM referral_withdrawals w \
                 WHERE w.client_id = $1 AND w.currency = $2 AND w.status = 'paid'), 0)::bigint AS withdrawn, \
             COALESCE((SELECT sum(w.amount_minor) FROM referral_withdrawals w \
                 WHERE w.client_id = $1 AND w.currency = $2 AND w.status IN ('holding', 'pending')), 0)::bigint AS pending"
        ))
        .bind(referrer.0)
        .bind(currency.code())
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(ReferralTotals {
            invited: row.invited,
            invited_paid: row.invited_paid,
            earned: Money::new(row.earned, currency),
            withdrawn: Money::new(row.withdrawn, currency),
            pending: Money::new(row.pending, currency),
        })
    }

    pub async fn reaching(
        &mut self,
        target: ClientId,
        starts: &[ClientId],
    ) -> StoreResult<Vec<ClientId>> {
        if starts.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = starts.iter().map(|id| id.0).collect();
        let found: Vec<i64> = sqlx::query_scalar(
            "WITH RECURSIVE up (start, id) AS ( \
                 SELECT s, s FROM unnest($2::bigint[]) AS s \
                 UNION \
                 SELECT up.start, l.referrer_id FROM up JOIN referral_links l ON l.client_id = up.id \
             ) SELECT DISTINCT start FROM up WHERE id = $1 ORDER BY start",
        )
        .bind(target.0)
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(found.into_iter().map(ClientId).collect())
    }

    pub async fn set(
        &mut self,
        client: ClientId,
        referrer: ClientId,
        by: Option<AdminId>,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "INSERT INTO referral_links (client_id, referrer_id, created_at, set_by, set_at) \
             SELECT c.id, $2, c.created_at, $3, now() FROM clients c WHERE c.id = $1 \
             ON CONFLICT (client_id) DO UPDATE SET referrer_id = EXCLUDED.referrer_id, \
             created_at = EXCLUDED.created_at, set_by = EXCLUDED.set_by, set_at = EXCLUDED.set_at",
        )
        .bind(client.0)
        .bind(referrer.0)
        .bind(by.map(|a| a.0))
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn remove(&mut self, client: ClientId) -> StoreResult<bool> {
        let done = sqlx::query("DELETE FROM referral_links WHERE client_id = $1")
            .bind(client.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
