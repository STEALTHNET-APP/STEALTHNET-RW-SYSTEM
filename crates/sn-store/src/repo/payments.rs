use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{
    AdminId, ClientId, LedgerEntryId, OrderId, PaymentId, RefundId, SubscriptionId,
};
use sn_domain::jobs::JobPayload;
use sn_domain::money::{Amount, PayCurrency};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{
    AUTORENEW_METHOD, NewPayment, Payment, PaymentStatus, ProviderId, RefundSubscriptionAction,
};
use sn_domain::pricing::BonusTerms;
use sqlx::PgConnection;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatAction {
    Balance,
    Review,
}

impl RepeatAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            RepeatAction::Balance => "balance",
            RepeatAction::Review => "review",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "balance" => Ok(RepeatAction::Balance),
            "review" => Ok(RepeatAction::Review),
            other => Err(StoreError::Decode(format!("повторная оплата: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRefundStatus {
    Pending,
    Succeeded,
    Declined,
}

impl ProviderRefundStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderRefundStatus::Pending => "pending",
            ProviderRefundStatus::Succeeded => "succeeded",
            ProviderRefundStatus::Declined => "declined",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "pending" => Ok(ProviderRefundStatus::Pending),
            "succeeded" => Ok(ProviderRefundStatus::Succeeded),
            "declined" => Ok(ProviderRefundStatus::Declined),
            other => Err(StoreError::Decode(format!("статус возврата: {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundActionStatus {
    None,
    Pending,
    Done,
    Failed,
}

impl RefundActionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            RefundActionStatus::None => "none",
            RefundActionStatus::Pending => "pending",
            RefundActionStatus::Done => "done",
            RefundActionStatus::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "none" => Ok(RefundActionStatus::None),
            "pending" => Ok(RefundActionStatus::Pending),
            "done" => Ok(RefundActionStatus::Done),
            "failed" => Ok(RefundActionStatus::Failed),
            other => Err(StoreError::Decode(format!(
                "статус действия с подпиской: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundAction {
    pub status: RefundActionStatus,
    pub attempts: u32,
    pub next_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub base_expire_at: Option<DateTime<Utc>>,
    pub target_expire_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChargeReview {
    pub payment: Payment,
    pub reason: Option<String>,
    pub review_since: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ChargeReviewRow {
    #[sqlx(flatten)]
    payment: PaymentRow,
    reason: Option<String>,
    review_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedRefundAction {
    pub request: ProviderRefund,
    pub order_id: OrderId,
    pub client_id: ClientId,
    pub subscription_id: Option<SubscriptionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProviderRefund {
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub hold_entry_id: Option<LedgerEntryId>,
    pub bonus_minor: i64,
    pub requested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRefund {
    pub id: i64,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub admin_id: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub status: ProviderRefundStatus,
    pub refund_id: Option<RefundId>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub hold_entry_id: Option<LedgerEntryId>,
    pub bonus_minor: i64,
    pub action: RefundAction,
    pub polled_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct ProviderRefundRow {
    id: i64,
    payment_id: i64,
    amount_minor: i64,
    currency: String,
    subscription_action: String,
    reason: Option<String>,
    admin_id: Option<i64>,
    provider_refund_id: Option<String>,
    status: String,
    refund_id: Option<i64>,
    failure_reason: Option<String>,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    hold_entry_id: Option<i64>,
    bonus_minor: i64,
    action_status: String,
    action_attempts: i32,
    action_next_at: Option<DateTime<Utc>>,
    action_error: Option<String>,
    action_base_expire_at: Option<DateTime<Utc>>,
    action_target_expire_at: Option<DateTime<Utc>>,
    polled_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct FailedRefundActionRow {
    #[sqlx(flatten)]
    request: ProviderRefundRow,
    order_id: i64,
    client_id: i64,
    subscription_id: Option<i64>,
}

const REFUND_FIELDS: [&str; 22] = [
    "id",
    "payment_id",
    "amount_minor",
    "currency",
    "subscription_action",
    "reason",
    "admin_id",
    "provider_refund_id",
    "status",
    "refund_id",
    "failure_reason",
    "created_at",
    "resolved_at",
    "hold_entry_id",
    "bonus_minor",
    "action_status",
    "action_attempts",
    "action_next_at",
    "action_error",
    "action_base_expire_at",
    "action_target_expire_at",
    "polled_at",
];

fn refund_columns(prefix: &str) -> String {
    REFUND_FIELDS
        .iter()
        .map(|field| format!("{prefix}{field}"))
        .collect::<Vec<_>>()
        .join(", ")
}

const fn action_code(action: RefundSubscriptionAction) -> &'static str {
    match action {
        RefundSubscriptionAction::Keep => "keep",
        RefundSubscriptionAction::Cut => "cut",
        RefundSubscriptionAction::Disable => "disable",
    }
}

fn parse_action(value: &str) -> StoreResult<RefundSubscriptionAction> {
    match value {
        "keep" => Ok(RefundSubscriptionAction::Keep),
        "cut" => Ok(RefundSubscriptionAction::Cut),
        "disable" => Ok(RefundSubscriptionAction::Disable),
        other => Err(StoreError::Decode(format!("действие с подпиской: {other}"))),
    }
}

impl TryFrom<ProviderRefundRow> for ProviderRefund {
    type Error = StoreError;

    fn try_from(row: ProviderRefundRow) -> Result<Self, Self::Error> {
        Ok(ProviderRefund {
            id: row.id,
            payment_id: PaymentId(row.payment_id),
            amount: Amount::new(row.amount_minor, PayCurrency::parse(&row.currency)?),
            subscription_action: parse_action(&row.subscription_action)?,
            reason: row.reason,
            admin_id: row.admin_id.map(AdminId),
            provider_refund_id: row.provider_refund_id,
            status: ProviderRefundStatus::parse(&row.status)?,
            refund_id: row.refund_id.map(RefundId),
            failure_reason: row.failure_reason,
            created_at: row.created_at,
            resolved_at: row.resolved_at,
            hold_entry_id: row.hold_entry_id.map(LedgerEntryId),
            bonus_minor: row.bonus_minor,
            action: RefundAction {
                status: RefundActionStatus::parse(&row.action_status)?,
                attempts: super::non_negative_u32(row.action_attempts, "action_attempts")?,
                next_at: row.action_next_at,
                error: row.action_error,
                base_expire_at: row.action_base_expire_at,
                target_expire_at: row.action_target_expire_at,
            },
            polled_at: row.polled_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceUpdate {
    pub pay_url: Option<String>,
    pub provider_txid: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaymentBonus {
    pub terms: Option<BonusTerms>,
    pub credited_minor: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaymentOutcome {
    pub refunded_minor: i64,
    pub bonus_minor: i64,
    pub repeat: Option<RepeatAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusChange {
    Applied(Payment),
    Unchanged(Payment),
}

impl StatusChange {
    pub fn payment(&self) -> &Payment {
        match self {
            StatusChange::Applied(p) | StatusChange::Unchanged(p) => p,
        }
    }

    pub const fn applied(&self) -> bool {
        matches!(self, StatusChange::Applied(_))
    }
}

#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: i64,
    public_id: Uuid,
    order_id: i64,
    client_id: i64,
    provider: String,
    method: Option<String>,
    amount_minor: i64,
    currency: String,
    status: String,
    provider_txid: Option<String>,
    pay_url: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    paid_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    pay_method: Option<String>,
}

const COLUMNS: &str = "id, public_id, order_id, client_id, provider, method, amount_minor, currency, status, \
                       provider_txid, pay_url, expires_at, paid_at, created_at, pay_method";

pub(crate) const TRANSITION_ATTEMPTS: usize = 4;

pub fn poll_key_prefix() -> String {
    let probe = JobPayload::PollPayment {
        payment_id: PaymentId(0),
    }
    .dedupe_key()
    .unwrap_or_default();
    probe.strip_suffix('0').unwrap_or(&probe).to_string()
}

pub fn charge_review_filter(alias: &str) -> String {
    format!(
        "{alias}.method = '{AUTORENEW_METHOD}' AND {alias}.status = 'pending' \
         AND ({alias}.expires_at IS NULL OR {alias}.expires_at > now()) \
         AND (SELECT j.status FROM jobs j WHERE j.dedupe_key = '{}' || {alias}.id::text \
             ORDER BY j.id DESC LIMIT 1) = 'review'",
        poll_key_prefix()
    )
}

const CHAIN_WATCHED: &str = "provider_txid IS NOT NULL AND (status = 'pending' \
     OR (status = 'canceled' AND (failure_reason = 'expired' \
     OR failure_reason = 'счёт заменён новым счётом с тем же промокодом' \
     OR failure_reason = 'счёт отменён вручную в админке') AND expires_at > $2))";

pub const TXID_CONSTRAINT: &str = "payments_provider_txid_key";

impl TryFrom<PaymentRow> for Payment {
    type Error = StoreError;

    fn try_from(row: PaymentRow) -> Result<Self, Self::Error> {
        Ok(Payment {
            id: PaymentId(row.id),
            public_id: row.public_id,
            order_id: OrderId(row.order_id),
            client_id: ClientId(row.client_id),
            provider: ProviderId(row.provider),
            method: row.method,
            pay_method: row.pay_method,
            amount: Amount::new(row.amount_minor, PayCurrency::parse(&row.currency)?),
            status: PaymentStatus::parse(&row.status)?,
            provider_txid: row.provider_txid,
            pay_url: row.pay_url,
            expires_at: row.expires_at,
            paid_at: row.paid_at,
            created_at: row.created_at,
        })
    }
}

pub struct PaymentsRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> PaymentsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        PaymentsRepo { conn }
    }

    pub async fn set_pay_method(&mut self, id: PaymentId, method: &str) -> StoreResult<()> {
        sqlx::query("UPDATE payments SET pay_method = $2 WHERE id = $1")
            .bind(id.0)
            .bind(method)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn create(&mut self, payment: &NewPayment) -> StoreResult<Payment> {
        if payment.amount.minor < 0 {
            return Err(StoreError::Invalid("сумма платежа отрицательная".into()));
        }
        let row: PaymentRow = sqlx::query_as(&format!(
            "INSERT INTO payments (order_id, client_id, provider, method, amount_minor, currency, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLUMNS}"
        ))
        .bind(payment.order_id.0)
        .bind(payment.client_id.0)
        .bind(payment.provider.as_str())
        .bind(&payment.method)
        .bind(payment.amount.minor)
        .bind(payment.amount.currency.code())
        .bind(payment.expires_at)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    async fn find_one(&mut self, condition: &str, id: i64) -> StoreResult<Payment> {
        let row: PaymentRow =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM payments WHERE {condition}"))
                .bind(id)
                .fetch_optional(&mut *self.conn)
                .await?
                .ok_or(StoreError::NotFound("платёж"))?;
        row.try_into()
    }

    pub async fn get(&mut self, id: PaymentId) -> StoreResult<Payment> {
        self.find_one("id = $1", id.0).await
    }

    pub async fn lock(&mut self, id: PaymentId) -> StoreResult<Payment> {
        self.find_one("id = $1 FOR UPDATE", id.0).await
    }

    pub async fn by_public_id(&mut self, public_id: Uuid) -> StoreResult<Payment> {
        let row: PaymentRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE public_id = $1"
        ))
        .bind(public_id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("платёж"))?;
        row.try_into()
    }

    pub async fn by_provider_txid(
        &mut self,
        provider: &str,
        txid: &str,
    ) -> StoreResult<Option<Payment>> {
        let row: Option<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE provider = $1 AND provider_txid = $2"
        ))
        .bind(provider)
        .bind(txid)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn attach_invoice(
        &mut self,
        id: PaymentId,
        invoice: &InvoiceUpdate,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE payments SET pay_url = $2, provider_txid = COALESCE($3, provider_txid), \
             expires_at = COALESCE($4, expires_at), provider_payload = $5 WHERE id = $1",
        )
        .bind(id.0)
        .bind(&invoice.pay_url)
        .bind(&invoice.provider_txid)
        .bind(invoice.expires_at)
        .bind(&invoice.payload)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn transition(
        &mut self,
        id: PaymentId,
        to: PaymentStatus,
        provider_txid: Option<&str>,
        reason: Option<&str>,
    ) -> StoreResult<StatusChange> {
        let mut current = self.lock(id).await?;
        for _ in 0..TRANSITION_ATTEMPTS {
            if let (Some(known), Some(incoming)) = (current.provider_txid.as_deref(), provider_txid)
                && known != incoming
            {
                return Err(StoreError::Conflict {
                    constraint: Some(TXID_CONSTRAINT.into()),
                    message: format!("у платежа уже другая транзакция провайдера: {known}"),
                });
            }
            if !current.status.transition(to)? {
                return Ok(StatusChange::Unchanged(current));
            }
            let row: Option<PaymentRow> = sqlx::query_as(&format!(
                "UPDATE payments SET status = $2, provider_txid = COALESCE(provider_txid, $3), failure_reason = $4, \
                 paid_at = CASE WHEN $2 = 'paid' THEN COALESCE(paid_at, now()) ELSE paid_at END \
                 WHERE id = $1 AND status = $5 RETURNING {COLUMNS}"
            ))
            .bind(id.0)
            .bind(to.as_str())
            .bind(provider_txid)
            .bind(reason)
            .bind(current.status.as_str())
            .fetch_optional(&mut *self.conn)
            .await?;
            match row {
                Some(row) => return Ok(StatusChange::Applied(row.try_into()?)),
                None => current = self.lock(id).await?,
            }
        }
        Err(StoreError::Conflict {
            constraint: None,
            message: "статус платежа меняется параллельно, повторите".into(),
        })
    }

    pub async fn charges_in_review(&mut self) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT count(*) FROM payments p WHERE {}",
            charge_review_filter("p")
        ))
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn charges_review_list(&mut self, limit: i64) -> StoreResult<Vec<ChargeReview>> {
        let rows: Vec<ChargeReviewRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS}, j.last_error AS reason, j.updated_at AS review_since \
             FROM payments p LEFT JOIN LATERAL (SELECT last_error, updated_at FROM jobs \
                 WHERE dedupe_key = '{}' || p.id::text ORDER BY id DESC LIMIT 1) j ON true \
             WHERE {} ORDER BY p.id DESC LIMIT $1",
            poll_key_prefix(),
            charge_review_filter("p")
        ))
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ChargeReview {
                    payment: row.payment.try_into()?,
                    reason: row.reason,
                    review_since: row.review_since,
                })
            })
            .collect()
    }

    pub async fn charge_reviews(
        &mut self,
        ids: &[PaymentId],
    ) -> StoreResult<HashMap<PaymentId, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(&format!(
            "SELECT p.id, (SELECT j.last_error FROM jobs j WHERE j.dedupe_key = '{}' || p.id::text \
                 ORDER BY j.id DESC LIMIT 1) \
             FROM payments p WHERE p.id = ANY($1) AND {}",
            poll_key_prefix(),
            charge_review_filter("p")
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, reason)| (PaymentId(id), reason.unwrap_or_default()))
            .collect())
    }

    pub async fn failure_reasons(
        &mut self,
        ids: &[PaymentId],
    ) -> StoreResult<HashMap<PaymentId, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, btrim(failure_reason) FROM payments \
             WHERE id = ANY($1) AND status = 'failed' AND NULLIF(btrim(failure_reason), '') IS NOT NULL",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, reason)| (PaymentId(id), reason))
            .collect())
    }

    pub async fn by_ids(&mut self, ids: &[PaymentId]) -> StoreResult<Vec<Payment>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE id = ANY($1) ORDER BY id"
        ))
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn latest_waiting(
        &mut self,
        client_id: ClientId,
        provider: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<Payment>> {
        let row: Option<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE client_id = $1 AND provider = $2 \
             AND status = 'pending' AND (expires_at IS NULL OR expires_at > $3) \
             ORDER BY id DESC LIMIT 1"
        ))
        .bind(client_id.0)
        .bind(provider)
        .bind(now)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn latest_in_series(
        &mut self,
        provider: &str,
        prefix: &str,
    ) -> StoreResult<Option<Payment>> {
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let row: Option<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE provider = $1 AND provider_txid LIKE $2 ESCAPE '\\' \
             AND status IN ('paid', 'refunded', 'partially_refunded') ORDER BY id DESC LIMIT 1"
        ))
        .bind(provider)
        .bind(pattern)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn for_order(&mut self, order_id: OrderId) -> StoreResult<Vec<Payment>> {
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE order_id = $1 ORDER BY id"
        ))
        .bind(order_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn for_client(
        &mut self,
        client_id: ClientId,
        page: PageRequest,
    ) -> StoreResult<Page<Payment>> {
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE client_id = $1 AND ($2::bigint IS NULL OR id < $2) ORDER BY id DESC LIMIT $3"
        ))
        .bind(client_id.0)
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<StoreResult<Vec<Payment>>>()?;
        Ok(Page::from_overfetch(items, page, |p| p.id.0))
    }

    pub async fn outcomes(
        &mut self,
        ids: &[PaymentId],
    ) -> StoreResult<HashMap<PaymentId, PaymentOutcome>> {
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<(i64, i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT p.id, p.refunded_minor, \
             p.bonus_minor - CASE WHEN p.bonus_minor = 0 THEN 0 ELSE \
               (SELECT COALESCE(sum(f.bonus_minor), 0)::bigint FROM refunds f WHERE f.payment_id = p.id) END, \
             r.action FROM payments p \
             LEFT JOIN payment_repeats r ON r.payment_id = p.id WHERE p.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|(id, refunded_minor, bonus_minor, repeat)| {
                Ok((
                    PaymentId(id),
                    PaymentOutcome {
                        refunded_minor,
                        bonus_minor,
                        repeat: repeat.as_deref().map(RepeatAction::parse).transpose()?,
                    },
                ))
            })
            .collect()
    }

    pub async fn chain_watch(
        &mut self,
        providers: &[&str],
        late_since: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<Payment>> {
        let providers: Vec<String> = providers.iter().map(|p| (*p).to_string()).collect();
        let rows: Vec<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE provider = ANY($1) AND {CHAIN_WATCHED} \
             ORDER BY id DESC LIMIT $3"
        ))
        .bind(&providers)
        .bind(late_since)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn chain_candidate(
        &mut self,
        id: PaymentId,
        late_since: DateTime<Utc>,
    ) -> StoreResult<Option<Payment>> {
        let row: Option<PaymentRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM payments WHERE id = $1 AND {CHAIN_WATCHED}"
        ))
        .bind(id.0)
        .bind(late_since)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn expire_stale(
        &mut self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<PaymentId>> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "UPDATE payments SET status = 'canceled', failure_reason = 'expired' WHERE id IN ( \
             SELECT id FROM payments WHERE status = 'pending' AND expires_at < $1 ORDER BY expires_at LIMIT $2 \
             FOR UPDATE SKIP LOCKED) RETURNING id",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(ids.into_iter().map(PaymentId).collect())
    }

    pub async fn remember_txid(&mut self, id: PaymentId, provider_txid: &str) -> StoreResult<bool> {
        let updated: Option<i64> = sqlx::query_scalar(
            "UPDATE payments SET provider_txid = $2 WHERE id = $1 \
             AND status IN ('pending', 'failed', 'canceled') \
             AND (provider_txid IS NULL OR provider_txid = $2) RETURNING id",
        )
        .bind(id.0)
        .bind(provider_txid)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(updated.is_some())
    }

    pub async fn mark_repeat(
        &mut self,
        id: PaymentId,
        original: Option<PaymentId>,
        action: RepeatAction,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "INSERT INTO payment_repeats (payment_id, original_payment_id, action) VALUES ($1, $2, $3) \
             ON CONFLICT (payment_id) DO NOTHING",
        )
        .bind(id.0)
        .bind(original.map(|p| p.0))
        .bind(action.as_str())
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn repeat_action(&mut self, id: PaymentId) -> StoreResult<Option<RepeatAction>> {
        let action: Option<String> =
            sqlx::query_scalar("SELECT action FROM payment_repeats WHERE payment_id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?;
        action.as_deref().map(RepeatAction::parse).transpose()
    }

    pub async fn create_provider_refund(
        &mut self,
        refund: &NewProviderRefund,
    ) -> StoreResult<ProviderRefund> {
        if refund.amount.minor <= 0 {
            return Err(StoreError::Invalid(
                "сумма возврата должна быть больше нуля".into(),
            ));
        }
        let row: ProviderRefundRow = sqlx::query_as(&format!(
            "INSERT INTO provider_refunds (payment_id, amount_minor, currency, subscription_action, reason, \
             admin_id, provider_refund_id, created_at, hold_entry_id, bonus_minor) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             RETURNING {}",
            refund_columns("")
        ))
        .bind(refund.payment_id.0)
        .bind(refund.amount.minor)
        .bind(refund.amount.currency.code())
        .bind(action_code(refund.subscription_action))
        .bind(&refund.reason)
        .bind(refund.admin_id.map(|a| a.0))
        .bind(&refund.provider_refund_id)
        .bind(refund.requested_at)
        .bind(refund.hold_entry_id.map(|e| e.0))
        .bind(refund.bonus_minor)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    pub async fn set_provider_refund_key(&mut self, id: i64, key: Uuid) -> StoreResult<()> {
        sqlx::query(
            "UPDATE provider_refunds SET request_key = $2 WHERE id = $1 \
             AND NOT EXISTS (SELECT 1 FROM provider_refunds WHERE request_key = $2)",
        )
        .bind(id)
        .bind(key)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn provider_refund_by_key(
        &mut self,
        key: Uuid,
    ) -> StoreResult<Option<ProviderRefund>> {
        let row: Option<ProviderRefundRow> = sqlx::query_as(&format!(
            "SELECT {} FROM provider_refunds WHERE request_key = $1",
            refund_columns("")
        ))
        .bind(key)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn hold(&mut self, id: PaymentId) -> StoreResult<bool> {
        let held: Option<i64> = sqlx::query_scalar(
            "UPDATE payments SET expires_at = NULL WHERE id = $1 AND status = 'pending' RETURNING id",
        )
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(held.is_some())
    }

    pub async fn awaiting_payment(&mut self, order_id: OrderId) -> StoreResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM payments WHERE order_id = $1 AND status = 'pending') \
             AND NOT EXISTS (SELECT 1 FROM payments WHERE order_id = $1 \
             AND status IN ('paid', 'refunded', 'partially_refunded'))",
        )
        .bind(order_id.0)
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn unheld_refunds_minor(
        &mut self,
        client_id: ClientId,
        currency: PayCurrency,
    ) -> StoreResult<i64> {
        let minor: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(r.amount_minor), 0)::bigint FROM provider_refunds r \
             JOIN payments p ON p.id = r.payment_id \
             JOIN orders o ON o.id = p.order_id \
             LEFT JOIN payment_repeats pr ON pr.payment_id = p.id \
             WHERE p.client_id = $1 AND r.currency = $2 AND r.status = 'pending' AND r.hold_entry_id IS NULL \
               AND (pr.action = 'balance' OR (pr.payment_id IS NULL AND o.kind = 'topup'))",
        )
        .bind(client_id.0)
        .bind(currency.code())
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(minor)
    }

    pub async fn promise_bonus(&mut self, id: PaymentId, terms: &BonusTerms) -> StoreResult<()> {
        sqlx::query("UPDATE payments SET bonus_terms = $2 WHERE id = $1")
            .bind(id.0)
            .bind(sqlx::types::Json(terms))
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn bonus_credited(&mut self, id: PaymentId, minor: i64) -> StoreResult<()> {
        sqlx::query("UPDATE payments SET bonus_minor = $2 WHERE id = $1")
            .bind(id.0)
            .bind(minor)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn bonus(&mut self, id: PaymentId) -> StoreResult<PaymentBonus> {
        let (terms, credited_minor): (Option<sqlx::types::Json<BonusTerms>>, i64) =
            sqlx::query_as("SELECT bonus_terms, bonus_minor FROM payments WHERE id = $1")
                .bind(id.0)
                .fetch_optional(&mut *self.conn)
                .await?
                .ok_or(StoreError::NotFound("платёж"))?;
        Ok(PaymentBonus {
            terms: terms.map(|terms| terms.0),
            credited_minor,
        })
    }

    pub async fn held_refunds_minor(
        &mut self,
        client_id: ClientId,
        currency: PayCurrency,
    ) -> StoreResult<i64> {
        let minor: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(r.amount_minor + r.bonus_minor), 0)::bigint FROM provider_refunds r \
             JOIN payments p ON p.id = r.payment_id \
             WHERE p.client_id = $1 AND r.currency = $2 AND r.status = 'pending' \
               AND r.hold_entry_id IS NOT NULL",
        )
        .bind(client_id.0)
        .bind(currency.code())
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(minor)
    }

    pub async fn provider_refunds(
        &mut self,
        payment_id: PaymentId,
    ) -> StoreResult<Vec<ProviderRefund>> {
        let rows: Vec<ProviderRefundRow> = sqlx::query_as(&format!(
            "SELECT {} FROM provider_refunds WHERE payment_id = $1 ORDER BY id",
            refund_columns("")
        ))
        .bind(payment_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn pending_provider_refunds(
        &mut self,
        requested_before: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<ProviderRefund>> {
        let rows: Vec<ProviderRefundRow> = sqlx::query_as(&format!(
            "SELECT {} FROM provider_refunds WHERE status = 'pending' AND created_at <= $1 \
             ORDER BY polled_at NULLS FIRST, created_at, id LIMIT $2",
            refund_columns("")
        ))
        .bind(requested_before)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn claim_refunds_to_poll(
        &mut self,
        providers: &[String],
        requested_before: DateTime<Utc>,
        polled_before: DateTime<Utc>,
        now: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<ProviderRefund>> {
        if providers.is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }
        let rows: Vec<ProviderRefundRow> = sqlx::query_as(&format!(
            "UPDATE provider_refunds SET polled_at = $4 WHERE id IN ( \
             SELECT r.id FROM provider_refunds r JOIN payments p ON p.id = r.payment_id \
             WHERE r.status = 'pending' AND r.provider_refund_id IS NOT NULL AND r.created_at <= $2 \
               AND (r.polled_at IS NULL OR r.polled_at <= $3) AND p.provider = ANY($1) \
             ORDER BY r.polled_at NULLS FIRST, r.created_at, r.id LIMIT $5 \
             FOR UPDATE OF r SKIP LOCKED) RETURNING {}",
            refund_columns("")
        ))
        .bind(providers)
        .bind(requested_before)
        .bind(polled_before)
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        let mut claimed = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<StoreResult<Vec<ProviderRefund>>>()?;
        claimed.sort_by_key(|r| r.id);
        Ok(claimed)
    }

    pub async fn provider_refund(&mut self, id: i64) -> StoreResult<ProviderRefund> {
        let row: ProviderRefundRow = sqlx::query_as(&format!(
            "SELECT {} FROM provider_refunds WHERE id = $1",
            refund_columns("")
        ))
        .bind(id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("заявка на возврат"))?;
        row.try_into()
    }

    pub async fn arm_refund_action(&mut self, id: i64, at: DateTime<Utc>) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE provider_refunds SET action_status = 'pending', action_attempts = 0, action_next_at = $2, \
             action_error = NULL WHERE id = $1 AND status = 'succeeded' AND subscription_action <> 'keep' \
             AND action_status IN ('none', 'failed')",
        )
        .bind(id)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn due_refund_actions(
        &mut self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> StoreResult<Vec<ProviderRefund>> {
        let rows: Vec<ProviderRefundRow> = sqlx::query_as(&format!(
            "SELECT {} FROM provider_refunds WHERE action_status = 'pending' AND action_next_at <= $1 \
             ORDER BY action_next_at, id LIMIT $2",
            refund_columns("")
        ))
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn plan_refund_action(
        &mut self,
        id: i64,
        base_expire_at: DateTime<Utc>,
        target_expire_at: DateTime<Utc>,
    ) -> StoreResult<(DateTime<Utc>, DateTime<Utc>)> {
        let plan: Option<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
            "UPDATE provider_refunds SET \
             action_target_expire_at = CASE WHEN action_base_expire_at IS NULL THEN $3 ELSE action_target_expire_at END, \
             action_base_expire_at = COALESCE(action_base_expire_at, $2) \
             WHERE id = $1 RETURNING action_base_expire_at, action_target_expire_at",
        )
        .bind(id)
        .bind(base_expire_at)
        .bind(target_expire_at)
        .fetch_optional(&mut *self.conn)
        .await?;
        plan.ok_or(StoreError::NotFound("заявка на возврат"))
    }

    pub async fn paid_after_refund(
        &mut self,
        id: i64,
        requested_action: &str,
    ) -> StoreResult<Option<OrderId>> {
        let found: Option<i64> = sqlx::query_scalar(
            "WITH req AS (SELECT p.order_id, refunded.subscription_id, COALESCE(( \
                 SELECT min(a.created_at) FROM audit_log a WHERE a.entity_kind = 'payment' \
                 AND a.entity_id = r.payment_id AND a.action = $2 \
                 AND a.details->>'request_id' = r.id::text), r.created_at) AS since \
               FROM provider_refunds r JOIN payments p ON p.id = r.payment_id \
               JOIN orders refunded ON refunded.id = p.order_id WHERE r.id = $1) \
             SELECT o.id FROM req JOIN orders o ON o.subscription_id = req.subscription_id \
             AND o.id <> req.order_id WHERE o.kind NOT IN ('topup', 'trial', 'admin_grant') \
             AND o.status IN ('paid', 'review', 'fulfilled') AND o.paid_at > req.since \
             ORDER BY o.paid_at, o.id LIMIT 1",
        )
        .bind(id)
        .bind(requested_action)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(found.map(OrderId))
    }

    pub async fn finish_refund_action(&mut self, id: i64) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE provider_refunds SET action_status = 'done', action_next_at = NULL, action_error = NULL \
             WHERE id = $1 AND action_status IN ('pending', 'failed')",
        )
        .bind(id)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn fail_refund_action(
        &mut self,
        id: i64,
        error: &str,
        retry_at: Option<DateTime<Utc>>,
    ) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE provider_refunds SET action_attempts = action_attempts + 1, action_error = $2, \
             action_next_at = $3, action_status = CASE WHEN $3::timestamptz IS NULL THEN 'failed' ELSE 'pending' END \
             WHERE id = $1 AND action_status = 'pending'",
        )
        .bind(id)
        .bind(error)
        .bind(retry_at)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn failed_refund_actions(
        &mut self,
        limit: i64,
    ) -> StoreResult<Vec<FailedRefundAction>> {
        let rows: Vec<FailedRefundActionRow> = sqlx::query_as(&format!(
            "SELECT {}, p.order_id, p.client_id, o.subscription_id FROM provider_refunds r \
             JOIN payments p ON p.id = r.payment_id JOIN orders o ON o.id = p.order_id \
             WHERE r.action_status = 'failed' ORDER BY r.id DESC LIMIT $1",
            refund_columns("r.")
        ))
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(FailedRefundAction {
                    order_id: OrderId(row.order_id),
                    client_id: ClientId(row.client_id),
                    subscription_id: row.subscription_id.map(SubscriptionId),
                    request: row.request.try_into()?,
                })
            })
            .collect()
    }

    pub async fn resolve_provider_refund(
        &mut self,
        id: i64,
        status: ProviderRefundStatus,
        refund_id: Option<RefundId>,
        failure_reason: Option<&str>,
        at: DateTime<Utc>,
    ) -> StoreResult<bool> {
        if status == ProviderRefundStatus::Pending {
            return Err(StoreError::Invalid(
                "ожидающий возврат завершается успехом или отказом".into(),
            ));
        }
        let done = sqlx::query(
            "UPDATE provider_refunds SET status = $2, refund_id = $3, failure_reason = $4, resolved_at = $5 \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(refund_id.map(|r| r.0))
        .bind(failure_reason)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::repo::NewClient;
    use crate::testing::test_store;
    use sn_domain::client::{Locale, RegistrationSource};
    use sn_domain::money::{Currency, Money};
    use sn_domain::order::{NewOrder, OrderKind, PurchaseIntent};
    use sn_domain::pricing::Quote;

    async fn pending(db: &mut impl Db, minor: i64) -> Payment {
        let client = db
            .clients()
            .create(&NewClient {
                locale: Locale::Ru,
                registration_source: RegistrationSource::Bot,
                note: None,
            })
            .await
            .unwrap()
            .id;
        let total = Money::new(minor, Currency::Usd);
        let order = db
            .orders()
            .create(&NewOrder {
                client_id: client,
                subscription_id: None,
                kind: OrderKind::TopUp,
                intent: PurchaseIntent::TopUp { amount: total },
                quote: Quote {
                    currency: Currency::Usd,
                    lines: Vec::new(),
                    subtotal: total,
                    discount: Money::zero(Currency::Usd),
                    total,
                    stars: None,
                    days: 0,
                    extra_devices: 0,
                },
                snapshot: None,
            })
            .await
            .unwrap();
        db.payments()
            .create(&NewPayment {
                order_id: order.id,
                client_id: client,
                provider: ProviderId::from("yoomoney"),
                method: None,
                amount: total.into(),
                expires_at: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn другая_транзакция_у_оплаченного_платежа_не_выдаётся_за_тот_же_платёж() {
        let Some(db) = test_store().await else { return };
        let mut conn = db.store.conn().await.unwrap();
        let payment = pending(&mut conn, 49_900).await;
        let paid = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-1"), None)
            .await
            .unwrap();
        assert!(paid.applied());
        let same = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-1"), None)
            .await
            .unwrap();
        assert!(!same.applied());
        let other = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-2"), None)
            .await;
        assert!(
            matches!(&other, Err(StoreError::Conflict { constraint: Some(c), .. }) if c == TXID_CONSTRAINT),
            "{other:?}"
        );
        conn.payments()
            .transition(payment.id, PaymentStatus::Refunded, None, None)
            .await
            .unwrap();
        let after_refund = conn
            .payments()
            .transition(payment.id, PaymentStatus::Paid, Some("op-3"), None)
            .await;
        assert!(
            matches!(&after_refund, Err(StoreError::Conflict { constraint: Some(c), .. }) if c == TXID_CONSTRAINT),
            "{after_refund:?}"
        );
        assert_eq!(
            conn.payments()
                .get(payment.id)
                .await
                .unwrap()
                .provider_txid
                .as_deref(),
            Some("op-1")
        );
        drop(conn);
        db.cleanup().await;
    }

    #[test]
    fn наблюдатель_цепочки_видит_заменённые_счета() {
        assert!(CHAIN_WATCHED.contains(sn_domain::payment::REPLACED_REASON));
        assert!(CHAIN_WATCHED.contains(sn_domain::payment::ADMIN_CANCELED_REASON));
    }
}
