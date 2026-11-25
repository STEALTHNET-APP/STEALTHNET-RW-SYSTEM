use crate::crypto::SealedSecret;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Duration, Utc};
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId, RefundId, SubscriptionId};
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::payment::{Refund, RefundMethod, RefundSubscriptionAction};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedMethodRecord {
    pub id: i64,
    pub client_id: ClientId,
    pub provider: String,
    pub token: SealedSecret,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SavedMethodRow {
    id: i64,
    token_nonce: Vec<u8>,
    token_ciphertext: Vec<u8>,
    title: Option<String>,
    created_at: DateTime<Utc>,
}

pub struct SavedMethodsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> SavedMethodsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        SavedMethodsRepo { conn }
    }

    pub async fn save(
        &mut self,
        client_id: ClientId,
        provider: &str,
        token: &SealedSecret,
        title: Option<&str>,
    ) -> StoreResult<i64> {
        sqlx::query(
            "UPDATE saved_payment_methods SET revoked_at = now() WHERE client_id = $1 AND provider = $2 AND revoked_at IS NULL",
        )
        .bind(client_id.0)
        .bind(provider)
        .execute(&mut *self.conn)
        .await?;
        Ok(sqlx::query_scalar(
            "INSERT INTO saved_payment_methods (client_id, provider, token_nonce, token_ciphertext, title) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(client_id.0)
        .bind(provider)
        .bind(&token.nonce)
        .bind(&token.ciphertext)
        .bind(title)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn kept_since(
        &mut self,
        client_id: ClientId,
        provider: &str,
        since: DateTime<Utc>,
    ) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM saved_payment_methods WHERE client_id = $1 AND provider = $2 \
             AND (revoked_at IS NULL OR created_at >= $3))",
        )
        .bind(client_id.0)
        .bind(provider)
        .bind(since)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn active(
        &mut self,
        client_id: ClientId,
        provider: &str,
    ) -> StoreResult<Option<SavedMethodRecord>> {
        let row: Option<SavedMethodRow> = sqlx::query_as(
            "SELECT id, token_nonce, token_ciphertext, title, created_at FROM saved_payment_methods \
             WHERE client_id = $1 AND provider = $2 AND revoked_at IS NULL",
        )
        .bind(client_id.0)
        .bind(provider)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.map(|row| SavedMethodRecord {
            id: row.id,
            client_id,
            provider: provider.to_string(),
            token: SealedSecret {
                nonce: row.token_nonce,
                ciphertext: row.token_ciphertext,
            },
            title: row.title,
            created_at: row.created_at,
        }))
    }

    pub async fn revoke(&mut self, id: i64) -> StoreResult<()> {
        sqlx::query("UPDATE saved_payment_methods SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRefund {
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub method: RefundMethod,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub bonus_minor: i64,
}

fn refund_method(value: &str) -> StoreResult<RefundMethod> {
    match value {
        "balance" => Ok(RefundMethod::Balance),
        "provider" => Ok(RefundMethod::Provider),
        other => Err(StoreError::Decode(format!("способ возврата: {other}"))),
    }
}

fn refund_action(value: &str) -> StoreResult<RefundSubscriptionAction> {
    match value {
        "keep" => Ok(RefundSubscriptionAction::Keep),
        "cut" => Ok(RefundSubscriptionAction::Cut),
        "disable" => Ok(RefundSubscriptionAction::Disable),
        other => Err(StoreError::Decode(format!("действие с подпиской: {other}"))),
    }
}

#[derive(sqlx::FromRow)]
struct RefundRow {
    id: i64,
    payment_id: i64,
    amount_minor: i64,
    currency: String,
    method: String,
    subscription_action: String,
    reason: Option<String>,
    admin_id: Option<i64>,
    provider_refund_id: Option<String>,
    bonus_minor: i64,
    created_at: DateTime<Utc>,
}

pub struct RefundsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> RefundsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        RefundsRepo { conn }
    }

    pub async fn create(&mut self, refund: &NewRefund) -> StoreResult<RefundId> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO refunds (payment_id, amount_minor, currency, method, subscription_action, reason, admin_id, \
             provider_refund_id, bonus_minor) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(refund.payment_id.0)
        .bind(refund.amount.minor)
        .bind(refund.amount.currency.code())
        .bind(match refund.method {
            RefundMethod::Balance => "balance",
            RefundMethod::Provider => "provider",
        })
        .bind(match refund.subscription_action {
            RefundSubscriptionAction::Keep => "keep",
            RefundSubscriptionAction::Cut => "cut",
            RefundSubscriptionAction::Disable => "disable",
        })
        .bind(&refund.reason)
        .bind(refund.admin_id.map(|a| a.0))
        .bind(&refund.provider_refund_id)
        .bind(refund.bonus_minor)
        .fetch_one(&mut *self.conn)
        .await?;
        sqlx::query("UPDATE payments SET refunded_minor = refunded_minor + $2 WHERE id = $1")
            .bind(refund.payment_id.0)
            .bind(refund.amount.minor)
            .execute(&mut *self.conn)
            .await?;
        Ok(RefundId(id))
    }

    pub async fn set_request_key(&mut self, id: RefundId, key: Uuid) -> StoreResult<()> {
        sqlx::query(
            "UPDATE refunds SET request_key = $2 WHERE id = $1 \
             AND NOT EXISTS (SELECT 1 FROM refunds WHERE request_key = $2)",
        )
        .bind(id.0)
        .bind(key)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn relink(&mut self, id: RefundId, provider_refund_id: &str) -> StoreResult<()> {
        sqlx::query("UPDATE refunds SET provider_refund_id = $2 WHERE id = $1")
            .bind(id.0)
            .bind(provider_refund_id)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn by_request_key(
        &mut self,
        key: Uuid,
    ) -> StoreResult<Option<(RefundId, PaymentId)>> {
        let row: Option<(i64, i64)> =
            sqlx::query_as("SELECT id, payment_id FROM refunds WHERE request_key = $1")
                .bind(key)
                .fetch_optional(&mut *self.conn)
                .await?;
        Ok(row.map(|(id, payment)| (RefundId(id), PaymentId(payment))))
    }

    pub async fn for_payment(&mut self, payment_id: PaymentId) -> StoreResult<Vec<Refund>> {
        let rows: Vec<RefundRow> = sqlx::query_as(
            "SELECT id, payment_id, amount_minor, currency, method, subscription_action, reason, admin_id, \
             provider_refund_id, bonus_minor, created_at FROM refunds WHERE payment_id = $1 ORDER BY id",
        )
        .bind(payment_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(Refund {
                    id: RefundId(row.id),
                    payment_id: PaymentId(row.payment_id),
                    amount: Amount::new(row.amount_minor, PayCurrency::parse(&row.currency)?),
                    method: refund_method(&row.method)?,
                    subscription_action: refund_action(&row.subscription_action)?,
                    reason: row.reason,
                    admin_id: row.admin_id.map(AdminId),
                    provider_refund_id: row.provider_refund_id,
                    bonus_minor: row.bonus_minor,
                    created_at: row.created_at,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptResult {
    Charged,
    Insufficient,
    Failed,
    Skipped,
    Reminded,
}

impl AttemptResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            AttemptResult::Charged => "charged",
            AttemptResult::Insufficient => "insufficient",
            AttemptResult::Failed => "failed",
            AttemptResult::Skipped => "skipped",
            AttemptResult::Reminded => "reminded",
        }
    }
}

pub struct AutoRenewRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> AutoRenewRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        AutoRenewRepo { conn }
    }

    pub async fn record_attempt(
        &mut self,
        subscription_id: SubscriptionId,
        period_key: &str,
        attempt_no: u32,
        result: AttemptResult,
        order_id: Option<OrderId>,
        details: Option<&str>,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "INSERT INTO autorenew_attempts (subscription_id, period_key, attempt_no, result, order_id, details) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (subscription_id, period_key, attempt_no) DO NOTHING",
        )
        .bind(subscription_id.0)
        .bind(period_key)
        .bind(attempt_no as i32)
        .bind(result.as_str())
        .bind(order_id.map(|o| o.0))
        .bind(details)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn attempts(
        &mut self,
        subscription_id: SubscriptionId,
        period_key: &str,
    ) -> StoreResult<u32> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM autorenew_attempts WHERE subscription_id = $1 AND period_key = $2",
        )
        .bind(subscription_id.0)
        .bind(period_key)
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(count as u32)
    }

    pub async fn mark_notified(
        &mut self,
        client_id: ClientId,
        kind: &str,
        cycle_key: &str,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "INSERT INTO notifications_sent (client_id, kind, cycle_key) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(client_id.0)
        .bind(kind)
        .bind(cycle_key)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }
}

pub struct BotStateRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> BotStateRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        BotStateRepo { conn }
    }

    pub async fn get(&mut self, key: &str) -> StoreResult<Option<serde_json::Value>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM bot_state WHERE key = $1")
                .bind(key)
                .fetch_optional(&mut *self.conn)
                .await?,
        )
    }

    pub async fn get_aged(
        &mut self,
        key: &str,
    ) -> StoreResult<Option<(serde_json::Value, Duration)>> {
        let row: Option<(serde_json::Value, i64)> = sqlx::query_as(
            "SELECT value, (EXTRACT(EPOCH FROM clock_timestamp() - updated_at) * 1000)::bigint \
             FROM bot_state WHERE key = $1",
        )
        .bind(key)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.map(|(value, age)| (value, Duration::milliseconds(age))))
    }

    pub async fn set(&mut self, key: &str, value: &serde_json::Value) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO bot_state (key, value) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn dialog(&mut self, chat_id: i64) -> StoreResult<Option<serde_json::Value>> {
        Ok(sqlx::query_scalar(
            "SELECT state FROM bot_dialogs WHERE chat_id = $1 AND expires_at > now()",
        )
        .bind(chat_id)
        .fetch_optional(&mut *self.conn)
        .await?)
    }

    pub async fn set_dialog(
        &mut self,
        chat_id: i64,
        state: &serde_json::Value,
        expires_at: DateTime<Utc>,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO bot_dialogs (chat_id, state, expires_at) VALUES ($1, $2, $3) \
             ON CONFLICT (chat_id) DO UPDATE SET state = EXCLUDED.state, expires_at = EXCLUDED.expires_at, updated_at = now()",
        )
        .bind(chat_id)
        .bind(state)
        .bind(expires_at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn clear_dialog(&mut self, chat_id: i64) -> StoreResult<()> {
        sqlx::query("DELETE FROM bot_dialogs WHERE chat_id = $1")
            .bind(chat_id)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }
}
