use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::client::Locale;
use sn_domain::format;
use sn_domain::ids::{ClientId, OrderId, PaymentId, PeriodId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::lang;
use sn_domain::money::{Amount, Currency, Money};
use sn_domain::order::{Order, PurchaseIntent};
use sn_domain::payment::{NoticeKind, Payment, RECURRING_METHOD};
use sn_domain::plan::{Cycle, Grace};
use sn_domain::pricing::Quote;
use sn_domain::subscription::{AutoRenewState, Subscription};
use sn_domain::tariff::Tariff;
use sn_payments::{
    CheckoutMethod, Invoice, PaymentError, PaymentProvider, RenewalCancel, RenewalStatus,
};
use sn_store::{Db, NewPaymentNotice, Tx, audit_entity};
use std::sync::Arc;

pub(crate) mod legacy;
#[cfg(test)]
mod tests;

pub const CANCELLED_TEMPLATE: &str = "renewal.cancelled";
pub const TURNED_OFF_TEMPLATE: &str = "autorenew.disabled";
pub const QUICK_REPEAT_DAYS: i64 = 7;
const CHECK_AFTER_CHARGE_HOURS: i64 = 30;
const OVERDUE_RETRY_HOURS: i64 = 6;
const DAILY_CHECK_HOURS: i64 = 24;
const SWEEP_BATCH: i64 = 200;
const NOTICE_TEXT_LIMIT: usize = 1000;
const ENDED_SHOWN_DAYS: i64 = 45;
const CANCEL_RETRY_MINUTES: [i64; 6] = [1, 5, 15, 60, 180, 360];

pub(crate) const LINK_COLUMNS: &str = "id, provider, external_id, client_id, subscription_id, \
     first_payment_id, tariff_id, period_id, days, periodicity, every_text, name, \
     amount_minor, currency, email, status, cancel_source, cancel_attempts, cancel_error, \
     ended_at, paid_until, next_charge_at, created_at, legacy_invoice_id, charges_since, started_at";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkStatus {
    Pending,
    Active,
    Cancelling,
    Cancelled,
    Failed,
}

impl LinkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            LinkStatus::Pending => "pending",
            LinkStatus::Active => "active",
            LinkStatus::Cancelling => "cancelling",
            LinkStatus::Cancelled => "cancelled",
            LinkStatus::Failed => "failed",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "active" => LinkStatus::Active,
            "cancelling" => LinkStatus::Cancelling,
            "cancelled" => LinkStatus::Cancelled,
            "failed" => LinkStatus::Failed,
            _ => LinkStatus::Pending,
        }
    }

    pub const fn live(self) -> bool {
        matches!(self, LinkStatus::Active | LinkStatus::Cancelling)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelSource {
    Client,
    Operator,
    Provider,
    Refund,
    Tariff,
    Block,
    Chargeback,
    Gone,
}

impl CancelSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            CancelSource::Client => "client",
            CancelSource::Operator => "operator",
            CancelSource::Provider => "provider",
            CancelSource::Refund => "refund",
            CancelSource::Tariff => "tariff",
            CancelSource::Block => "block",
            CancelSource::Chargeback => "chargeback",
            CancelSource::Gone => "gone",
        }
    }

    const fn tells_client(self) -> bool {
        matches!(
            self,
            CancelSource::Operator | CancelSource::Provider | CancelSource::Tariff
        )
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct Link {
    pub id: i64,
    pub provider: String,
    pub external_id: String,
    pub client_id: i64,
    pub subscription_id: Option<i64>,
    pub first_payment_id: Option<i64>,
    pub tariff_id: Option<i64>,
    pub period_id: Option<i64>,
    pub days: i32,
    pub periodicity: String,
    pub every_text: String,
    pub name: String,
    pub amount_minor: i64,
    pub currency: String,
    pub email: String,
    pub status: String,
    pub cancel_source: Option<String>,
    pub cancel_attempts: i32,
    pub cancel_error: Option<String>,
    pub ended_at: Option<DateTime<Utc>>,
    pub paid_until: Option<DateTime<Utc>>,
    pub next_charge_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub legacy_invoice_id: Option<i64>,
    pub charges_since: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
}

impl Link {
    pub fn status(&self) -> LinkStatus {
        LinkStatus::parse(&self.status)
    }

    pub fn amount(&self) -> Option<Money> {
        Currency::parse(&self.currency)
            .ok()
            .map(|currency| Money::new(self.amount_minor, currency))
    }

    pub(crate) fn cycle(&self, services: &Services) -> Cycle {
        cycle_of(services, &self.provider, &self.periodicity, self.days)
    }
}

pub(crate) fn cycle_of(services: &Services, provider: &str, periodicity: &str, days: i32) -> Cycle {
    services
        .payments()
        .get(provider)
        .and_then(|p| p.renewal_cycle(periodicity))
        .unwrap_or_else(|| Cycle::Days(u32::try_from(days.max(1)).unwrap_or(30)))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenewalView {
    pub provider: String,
    pub title: String,
    pub name: String,
    pub status: LinkStatus,
    pub amount: Option<Money>,
    pub periodicity: String,
    pub every: String,
    pub days: u32,
    pub next_charge_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub charges: i64,
    pub email: String,
    pub cancel_source: Option<String>,
    pub ended_at: Option<DateTime<Utc>>,
    pub paid_until: Option<DateTime<Utc>>,
    pub problem: Option<String>,
    pub legacy: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SweepReport {
    pub checked: usize,
    pub cancelled: usize,
    pub charges: usize,
    pub failed: usize,
    pub adopted: usize,
    pub legacy_checked: usize,
    pub legacy_skipped: Option<&'static str>,
}

pub(crate) use sn_domain::client::mask_email;

pub(crate) fn no_email(title: &str, locale: Locale) -> String {
    match locale {
        Locale::En => format!(
            "the buyer email at {title} is unknown, so the subscription can only be cancelled in the {title} account"
        ),
        Locale::Ru => format!(
            "почта покупателя в {title} неизвестна, поэтому отменить подписку можно только в кабинете {title}"
        ),
    }
}

pub(crate) fn title_of(services: &Services, provider: &str) -> String {
    services
        .payments()
        .get(provider)
        .map_or_else(|| provider.to_string(), |p| p.title().to_string())
}

pub(crate) async fn link_by_id(db: &mut impl Db, id: i64) -> ServiceResult<Option<Link>> {
    Ok(sqlx::query_as::<_, Link>(&format!(
        "SELECT {LINK_COLUMNS} FROM provider_subscriptions WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db.conn())
    .await?)
}

pub(crate) async fn lock_link(tx: &mut Tx, id: i64) -> ServiceResult<Option<Link>> {
    Ok(sqlx::query_as::<_, Link>(&format!(
        "SELECT {LINK_COLUMNS} FROM provider_subscriptions WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(tx.conn())
    .await?)
}

pub(crate) async fn lock_series(
    tx: &mut Tx,
    provider: &str,
    series: &str,
) -> ServiceResult<Option<Link>> {
    Ok(sqlx::query_as::<_, Link>(&format!(
        "SELECT {LINK_COLUMNS} FROM provider_subscriptions \
         WHERE provider = $1 AND lower(external_id) = lower($2) FOR UPDATE"
    ))
    .bind(provider)
    .bind(series.trim())
    .fetch_optional(tx.conn())
    .await?)
}

pub(crate) async fn series_of_contract(
    db: &mut impl Db,
    provider: &str,
    contract: &str,
) -> ServiceResult<Option<i64>> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM provider_subscriptions WHERE provider = $1 AND lower(external_id) = lower($2) \
         UNION ALL SELECT c.provider_subscription_id FROM provider_subscription_charges c \
         JOIN provider_subscriptions ps ON ps.id = c.provider_subscription_id \
         WHERE ps.provider = $1 AND lower(c.external_id) = lower($2) LIMIT 1",
    )
    .bind(provider)
    .bind(contract.trim())
    .fetch_optional(db.conn())
    .await?)
}

pub(crate) async fn live_link(
    db: &mut impl Db,
    subscription: SubscriptionId,
) -> ServiceResult<Option<Link>> {
    Ok(sqlx::query_as::<_, Link>(&format!(
        "SELECT {LINK_COLUMNS} FROM provider_subscriptions \
         WHERE subscription_id = $1 AND status IN ('active', 'cancelling')"
    ))
    .bind(subscription.0)
    .fetch_optional(db.conn())
    .await?)
}

pub(crate) async fn last_charge(
    db: &mut impl Db,
    link: i64,
) -> ServiceResult<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT max(charged_at) FROM provider_subscription_charges WHERE provider_subscription_id = $1",
    )
    .bind(link)
    .fetch_one(db.conn())
    .await?)
}

async fn charges_of(db: &mut impl Db, link: i64) -> ServiceResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_subscription_charges WHERE provider_subscription_id = $1",
    )
    .bind(link)
    .fetch_one(db.conn())
    .await?)
}

pub(crate) async fn record_charge(
    tx: &mut Tx,
    link: i64,
    payment: PaymentId,
    order: OrderId,
    external_id: &str,
    first: bool,
    at: DateTime<Utc>,
) -> ServiceResult<()> {
    sqlx::query(
        "INSERT INTO provider_subscription_charges \
         (provider_subscription_id, payment_id, order_id, external_id, first, charged_at) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
    )
    .bind(link)
    .bind(payment.0)
    .bind(order.0)
    .bind(external_id)
    .bind(first)
    .bind(at)
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub(crate) fn next_check(next_charge: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    let due = next_charge + Duration::hours(CHECK_AFTER_CHARGE_HOURS);
    if due <= now {
        now + Duration::hours(OVERDUE_RETRY_HOURS)
    } else {
        due.min(now + Duration::hours(DAILY_CHECK_HOURS))
    }
}

pub(crate) async fn note_charge(
    tx: &mut Tx,
    link: &Link,
    cycle: Cycle,
    charged_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> ServiceResult<()> {
    let next = cycle.after(charged_at.min(now));
    sqlx::query(
        "UPDATE provider_subscriptions SET next_charge_at = $2, check_at = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(link.id)
    .bind(next)
    .bind(next_check(next, now))
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub(crate) async fn audit(
    db: &mut impl Db,
    actor: Actor,
    action: &str,
    link: &Link,
    details: serde_json::Value,
) -> ServiceResult<()> {
    let (kind, id) = match link.subscription_id {
        Some(id) => (audit_entity::SUBSCRIPTION, id),
        None => (audit_entity::CLIENT, link.client_id),
    };
    let mut details = details;
    if let Some(map) = details.as_object_mut() {
        map.insert("provider".into(), link.provider.clone().into());
        map.insert("contract".into(), link.external_id.clone().into());
        map.insert("client_id".into(), link.client_id.into());
        if let Some(invoice) = link.legacy_invoice_id {
            map.insert("legacy_invoice".into(), invoice.into());
        }
    }
    db.audit()
        .record(&actor.audit(action, kind, Some(id), details))
        .await?;
    Ok(())
}

pub(crate) async fn notice(
    db: &mut impl Db,
    link: &Link,
    key: String,
    kind: NoticeKind,
    amount: Option<Amount>,
    text: String,
) -> ServiceResult<()> {
    db.payment_notices()
        .record(&NewPaymentNotice {
            provider: link.provider.clone(),
            event_key: key,
            kind,
            payment_id: link.first_payment_id.map(PaymentId),
            client_id: Some(ClientId(link.client_id)),
            amount,
            text: text.chars().take(NOTICE_TEXT_LIMIT).collect(),
        })
        .await?;
    Ok(())
}

pub(crate) async fn notify(
    db: &mut impl Db,
    client: i64,
    template: &str,
    params: serde_json::Value,
) -> ServiceResult<()> {
    db.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id: ClientId(client),
                template: template.into(),
                params,
            },
            None,
        )
        .await?;
    Ok(())
}

async fn expire_of(db: &mut impl Db, link: &Link) -> ServiceResult<Option<DateTime<Utc>>> {
    Ok(match link.subscription_id {
        Some(id) => db
            .subscriptions()
            .get(SubscriptionId(id))
            .await
            .ok()
            .and_then(|s| s.mirror.expire_at),
        None => None,
    })
}

pub(crate) async fn close(
    tx: &mut Tx,
    link: &Link,
    status: LinkStatus,
    source: Option<CancelSource>,
    actor: Actor,
    until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> ServiceResult<bool> {
    if !link.status().live() && link.status() != LinkStatus::Pending {
        return Ok(false);
    }
    let source = link
        .cancel_source
        .clone()
        .or_else(|| source.map(|s| s.as_str().to_string()));
    sqlx::query(
        "UPDATE provider_subscriptions SET status = $2, cancel_source = $3, ended_at = $4, \
         paid_until = COALESCE($5, paid_until), check_at = NULL, cancel_error = NULL, updated_at = now() \
         WHERE id = $1",
    )
    .bind(link.id)
    .bind(status.as_str())
    .bind(source.as_deref())
    .bind(now)
    .bind(until)
    .execute(tx.conn())
    .await?;
    if link.status() == LinkStatus::Pending {
        return Ok(true);
    }
    if let Some(id) = link.subscription_id {
        mark_autorenew(tx, SubscriptionId(id), false).await?;
    }
    let action = match status {
        LinkStatus::Failed => "subscription.renewal_failed",
        _ => "subscription.renewal_cancelled",
    };
    audit(
        tx,
        actor,
        action,
        link,
        serde_json::json!({ "source": source, "until": until }),
    )
    .await?;
    let date = match expire_of(tx, link).await? {
        Some(at) => Some(at),
        None => until,
    };
    if status == LinkStatus::Failed {
        notify(
            tx,
            link.client_id,
            TURNED_OFF_TEMPLATE,
            serde_json::json!({ "subscription_id": link.subscription_id, "expire_at": date }),
        )
        .await?;
    } else if source
        .as_deref()
        .and_then(source_of)
        .is_some_and(CancelSource::tells_client)
    {
        notify(
            tx,
            link.client_id,
            CANCELLED_TEMPLATE,
            serde_json::json!({
                "subscription_id": link.subscription_id,
                "expire_at": date,
                "provider": link.provider,
            }),
        )
        .await?;
    }
    Ok(true)
}

fn source_of(raw: &str) -> Option<CancelSource> {
    Some(match raw {
        "client" => CancelSource::Client,
        "operator" => CancelSource::Operator,
        "provider" => CancelSource::Provider,
        "refund" => CancelSource::Refund,
        "tariff" => CancelSource::Tariff,
        "block" => CancelSource::Block,
        "chargeback" => CancelSource::Chargeback,
        "gone" => CancelSource::Gone,
        _ => return None,
    })
}

pub(crate) async fn request_cancel(
    tx: &mut Tx,
    link: &Link,
    source: CancelSource,
    actor: Actor,
    now: DateTime<Utc>,
) -> ServiceResult<bool> {
    if link.status() != LinkStatus::Active {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE provider_subscriptions SET status = 'cancelling', cancel_source = $2, \
         cancel_requested_at = $3, check_at = $3, updated_at = now() WHERE id = $1",
    )
    .bind(link.id)
    .bind(source.as_str())
    .bind(now)
    .execute(tx.conn())
    .await?;
    audit(
        tx,
        actor,
        "subscription.renewal_cancel_requested",
        link,
        serde_json::json!({ "source": source.as_str() }),
    )
    .await?;
    Ok(true)
}

pub(crate) async fn note_invoice(
    db: &mut impl Db,
    provider: &str,
    payment: PaymentId,
    order: &Order,
    invoice: &Invoice,
) -> ServiceResult<()> {
    let Some(recurring) = invoice.payload.get("recurring") else {
        return Ok(());
    };
    let Some(external) = invoice
        .external_id
        .as_deref()
        .filter(|e| !e.trim().is_empty())
    else {
        return Ok(());
    };
    let (tariff, period) = match &order.intent {
        PurchaseIntent::Subscription {
            tariff_id,
            period_id,
            ..
        } => (Some(tariff_id.0), Some(period_id.0)),
        _ => (None, None),
    };
    let Some(days) = order.snapshot.as_ref().map(|s| s.days) else {
        return Ok(());
    };
    let text = |key: &str| {
        recurring
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let Some(amount) = recurring.get("amount").and_then(serde_json::Value::as_i64) else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO provider_subscriptions (provider, external_id, client_id, subscription_id, \
         first_payment_id, tariff_id, period_id, days, offer_id, product_id, periodicity, \
         every_text, name, amount_minor, currency, email) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
         ON CONFLICT (provider, external_id) DO NOTHING",
    )
    .bind(provider)
    .bind(external.trim())
    .bind(order.client_id.0)
    .bind(order.subscription_id.map(|s| s.0))
    .bind(payment.0)
    .bind(tariff)
    .bind(period)
    .bind(i32::try_from(days).unwrap_or(i32::MAX))
    .bind(text("offer"))
    .bind(Some(text("product")).filter(|p| !p.is_empty()))
    .bind(text("periodicity"))
    .bind(text("every"))
    .bind(text("name"))
    .bind(amount)
    .bind(text("currency"))
    .bind(text("email"))
    .execute(db.conn())
    .await?;
    Ok(())
}

pub(crate) async fn activate(
    services: &Services,
    tx: &mut Tx,
    paid: &Payment,
    order: &Order,
    now: DateTime<Utc>,
) -> ServiceResult<()> {
    if paid.pay_method.as_deref() != Some(RECURRING_METHOD) {
        return Ok(());
    }
    let Some(link) = sqlx::query_as::<_, Link>(&format!(
        "SELECT {LINK_COLUMNS} FROM provider_subscriptions \
         WHERE first_payment_id = $1 AND status = 'pending' FOR UPDATE"
    ))
    .bind(paid.id.0)
    .fetch_optional(tx.conn())
    .await?
    else {
        return Ok(());
    };
    let paid_at = paid.paid_at.unwrap_or(now);
    let target = link.subscription_id.or(order.subscription_id.map(|s| s.0));
    let taken = match target {
        Some(id) => live_link(tx, SubscriptionId(id)).await?.is_some(),
        None => false,
    };
    let next = link.cycle(services).after(paid_at.min(now));
    sqlx::query(
        "UPDATE provider_subscriptions SET status = $2, subscription_id = $3, next_charge_at = $4, \
         check_at = $5, cancel_source = $6, cancel_requested_at = $7, updated_at = now() WHERE id = $1",
    )
    .bind(link.id)
    .bind(if taken { "cancelling" } else { "active" })
    .bind(if taken { None } else { target })
    .bind(next)
    .bind(if taken { now } else { next_check(next, now) })
    .bind(taken.then_some(CancelSource::Gone.as_str()))
    .bind(taken.then_some(now))
    .execute(tx.conn())
    .await?;
    record_charge(
        tx,
        link.id,
        paid.id,
        order.id,
        &link.external_id,
        true,
        paid_at,
    )
    .await?;
    let fresh = Link {
        subscription_id: if taken { None } else { target },
        ..link
    };
    audit(
        tx,
        Actor::System,
        "subscription.renewal_started",
        &fresh,
        serde_json::json!({
            "amount": fresh.amount(),
            "every": fresh.every_text,
            "periodicity": fresh.periodicity,
            "payment_id": paid.id,
            "duplicate": taken,
        }),
    )
    .await?;
    if taken {
        notice(
            tx,
            &fresh,
            format!("renewal_duplicate:{}", fresh.external_id),
            NoticeKind::ManualCredit,
            Some(paid.amount),
            format!(
                "Клиент оформил вторую подписку {} на ту же подписку панели. Оплата засчитана, \
                 а вторую подписку панель отменяет в {}, чтобы не списывать дважды",
                fresh.provider, fresh.provider
            ),
        )
        .await?;
        return Ok(());
    }
    if let Some(id) = target {
        mark_autorenew(tx, SubscriptionId(id), true).await?;
    }
    Ok(())
}

async fn mark_autorenew(tx: &mut Tx, id: SubscriptionId, enabled: bool) -> ServiceResult<()> {
    let subscription = match tx.subscriptions().get(id).await {
        Ok(subscription) => subscription,
        Err(sn_store::StoreError::NotFound(_)) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if subscription.autorenew.enabled == enabled {
        return Ok(());
    }
    tx.subscriptions()
        .set_autorenew(
            id,
            AutoRenewState {
                enabled,
                period_id: subscription.autorenew.period_id,
            },
        )
        .await?;
    Ok(())
}

pub(crate) async fn grace_for(
    services: &Services,
    db: &mut impl Db,
    order: OrderId,
) -> ServiceResult<Option<Grace>> {
    let found: Option<(bool, DateTime<Utc>, String, String, i32)> = sqlx::query_as(
        "SELECT c.first, c.charged_at, ps.provider, ps.periodicity, ps.days \
         FROM provider_subscription_charges c \
         JOIN provider_subscriptions ps ON ps.id = c.provider_subscription_id \
         WHERE c.order_id = $1 LIMIT 1",
    )
    .bind(order.0)
    .fetch_optional(db.conn())
    .await?;
    Ok(
        found.map(|(first, charged_at, provider, periodicity, days)| {
            let grace = if first {
                Grace::first()
            } else {
                Grace::renewal()
            };
            grace.every(
                Some(cycle_of(services, &provider, &periodicity, days)),
                Some(charged_at),
            )
        }),
    )
}

pub(crate) async fn after_fulfilled(
    tx: &mut Tx,
    order: &Order,
    now: DateTime<Utc>,
) -> ServiceResult<()> {
    let Some(subscription) = order.subscription_id else {
        return Ok(());
    };
    let first = sqlx::query_as::<_, Link>(&format!(
        "SELECT {} FROM provider_subscriptions ps \
         JOIN provider_subscription_charges c ON c.provider_subscription_id = ps.id \
         WHERE c.order_id = $1 AND c.first FOR UPDATE OF ps",
        LINK_COLUMNS
            .split(", ")
            .map(|c| format!("ps.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(order.id.0)
    .fetch_optional(tx.conn())
    .await?;
    if let Some(link) = &first
        && link.status() == LinkStatus::Active
        && link.subscription_id.is_none()
    {
        let taken = live_link(tx, subscription).await?;
        if taken.is_some() {
            sqlx::query(
                "UPDATE provider_subscriptions SET status = 'cancelling', cancel_source = 'gone', \
                 cancel_requested_at = $2, check_at = $2, updated_at = now() WHERE id = $1",
            )
            .bind(link.id)
            .bind(now)
            .execute(tx.conn())
            .await?;
        } else {
            sqlx::query(
                "UPDATE provider_subscriptions SET subscription_id = $2, updated_at = now() WHERE id = $1",
            )
            .bind(link.id)
            .bind(subscription.0)
            .execute(tx.conn())
            .await?;
            mark_autorenew(tx, subscription, true).await?;
        }
    } else if let Some(link) = &first
        && link.status() == LinkStatus::Active
        && link.subscription_id == Some(subscription.0)
    {
        mark_autorenew(tx, subscription, true).await?;
    }
    let tariff = match &order.intent {
        PurchaseIntent::Subscription { tariff_id, .. } => Some(tariff_id.0),
        _ => None,
    };
    if let Some(live) = live_link(tx, subscription).await?
        && live.status() == LinkStatus::Active
        && first.as_ref().is_none_or(|f| f.id != live.id)
        && tariff.is_some()
        && live.tariff_id != tariff
    {
        request_cancel(tx, &live, CancelSource::Tariff, Actor::System, now).await?;
    }
    Ok(())
}

pub(crate) struct RenewalCart<'c> {
    pub tariff: &'c Tariff,
    pub period: PeriodId,
    pub quote: &'c Quote,
    pub currency: Currency,
    pub extra_devices: u32,
    pub current: Option<&'c Subscription>,
}

pub struct Renewals<'a> {
    services: &'a Services,
}

impl<'a> Renewals<'a> {
    pub fn new(services: &'a Services) -> Self {
        Renewals { services }
    }

    fn provider(&self, id: &str) -> Option<Arc<dyn PaymentProvider>> {
        self.services.payments().get(id)
    }

    pub(crate) async fn offer_methods(
        &self,
        methods: Vec<CheckoutMethod>,
        cart: &RenewalCart<'_>,
    ) -> ServiceResult<Vec<CheckoutMethod>> {
        if !methods
            .iter()
            .any(|m| m.method.as_deref() == Some(RECURRING_METHOD))
        {
            return Ok(methods);
        }
        let busy = match cart.current {
            Some(current) => {
                current.extra_devices > 0 || {
                    let mut conn = self.services.store().conn().await?;
                    live_link(&mut conn, current.id).await?.is_some()
                }
            }
            None => false,
        };
        let price = cart
            .tariff
            .period(cart.period)
            .and_then(|p| p.price(cart.currency));
        let days = cart.tariff.period(cart.period).map(|p| p.days);
        let registry = self.services.payments();
        Ok(methods
            .into_iter()
            .filter_map(|mut method| {
                if method.method.as_deref() != Some(RECURRING_METHOD) {
                    return Some(method);
                }
                if busy || cart.extra_devices > 0 {
                    return None;
                }
                let provider = registry.get(method.provider)?;
                let config = registry.config(method.provider);
                if !provider.renewals_enabled(&config) {
                    return None;
                }
                let plan = provider.renewal_plans(&config).into_iter().find(|plan| {
                    plan.binding.tariff_id == cart.tariff.id.0
                        && plan.binding.period_id == cart.period.0
                        && plan.amount.currency == cart.currency
                })?;
                let days = days?;
                let fits = Some(plan.amount) == price
                    && cart.quote.total == plan.amount
                    && (plan.min_days..=plan.max_days).contains(&days);
                if !fits {
                    return None;
                }
                let every = sn_payments::english::shown(plan.every);
                method.hint = match lang::current() {
                    Locale::En => format!(
                        "{} now and {every} until you turn it off",
                        format::money(plan.amount)
                    ),
                    Locale::Ru => format!(
                        "{} сейчас и {every}, пока не отключите",
                        format::money(plan.amount)
                    ),
                };
                Some(method)
            })
            .collect())
    }

    pub async fn view(&self, subscription: SubscriptionId) -> ServiceResult<Option<RenewalView>> {
        let mut conn = self.services.store().conn().await?;
        self.view_in(&mut conn, subscription).await
    }

    pub(crate) async fn view_in(
        &self,
        db: &mut impl Db,
        subscription: SubscriptionId,
    ) -> ServiceResult<Option<RenewalView>> {
        let since = self.services.now() - Duration::days(ENDED_SHOWN_DAYS);
        let link = sqlx::query_as::<_, Link>(&format!(
            "SELECT {LINK_COLUMNS} FROM provider_subscriptions WHERE subscription_id = $1 \
             AND (status IN ('active', 'cancelling') OR (status IN ('cancelled', 'failed') AND ended_at > $2)) \
             ORDER BY (status IN ('active', 'cancelling')) DESC, id DESC LIMIT 1"
        ))
        .bind(subscription.0)
        .bind(since)
        .fetch_optional(db.conn())
        .await?;
        let Some(link) = link else {
            return Ok(None);
        };
        let charges = charges_of(db, link.id).await?;
        let title = title_of(self.services, &link.provider);
        let problem = link.cancel_error.clone().or_else(|| {
            (link.status().live() && link.email.trim().is_empty())
                .then(|| no_email(&title, lang::current()))
        });
        Ok(Some(RenewalView {
            title,
            name: link.name.clone(),
            provider: link.provider.clone(),
            status: link.status(),
            amount: link.amount(),
            periodicity: link.periodicity.clone(),
            every: link.every_text.clone(),
            days: u32::try_from(link.days).unwrap_or(0),
            next_charge_at: link
                .next_charge_at
                .filter(|_| link.status() == LinkStatus::Active),
            started_at: link.started_at.unwrap_or(link.created_at),
            charges,
            email: mask_email(&link.email),
            cancel_source: link.cancel_source.clone(),
            ended_at: link.ended_at,
            paid_until: link.paid_until,
            problem,
            legacy: link.charges_since.is_some(),
        }))
    }

    pub async fn cancel_subscription(
        &self,
        subscription: SubscriptionId,
        source: CancelSource,
        actor: Actor,
    ) -> ServiceResult<Option<RenewalView>> {
        let link = {
            let mut conn = self.services.store().conn().await?;
            live_link(&mut conn, subscription).await?
        };
        if let Some(link) = link {
            self.cancel_link(link.id, source, actor).await?;
        }
        self.view(subscription).await
    }

    pub async fn cancel_client(
        &self,
        client: ClientId,
        source: CancelSource,
        actor: Actor,
    ) -> ServiceResult<usize> {
        let ids: Vec<i64> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_scalar(
                "SELECT id FROM provider_subscriptions WHERE client_id = $1 \
                 AND status IN ('active', 'cancelling') ORDER BY id",
            )
            .bind(client.0)
            .fetch_all(conn.conn())
            .await?
        };
        for id in &ids {
            self.cancel_link(*id, source, actor).await?;
        }
        Ok(ids.len())
    }

    pub async fn cancel_for_payment(
        &self,
        payment: PaymentId,
        source: CancelSource,
    ) -> ServiceResult<bool> {
        let id: Option<i64> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_scalar(
                "SELECT ps.id FROM provider_subscriptions ps WHERE ps.first_payment_id = $1 \
                 AND ps.status IN ('active', 'cancelling') \
                 UNION ALL SELECT c.provider_subscription_id FROM provider_subscription_charges c \
                 JOIN provider_subscriptions ps ON ps.id = c.provider_subscription_id \
                 WHERE c.payment_id = $1 AND ps.status IN ('active', 'cancelling') LIMIT 1",
            )
            .bind(payment.0)
            .fetch_optional(conn.conn())
            .await?
        };
        match id {
            Some(id) => {
                self.cancel_link(id, source, Actor::System).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub async fn check_soon(&self, payment: PaymentId) -> ServiceResult<bool> {
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let updated = sqlx::query(
            "UPDATE provider_subscriptions SET check_at = $2, updated_at = now() \
             WHERE status = 'active' AND (first_payment_id = $1 OR id IN \
             (SELECT provider_subscription_id FROM provider_subscription_charges WHERE payment_id = $1))",
        )
        .bind(payment.0)
        .bind(now)
        .execute(conn.conn())
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    pub(crate) async fn cancel_link(
        &self,
        id: i64,
        source: CancelSource,
        actor: Actor,
    ) -> ServiceResult<()> {
        let now = self.services.now();
        let link = {
            let mut tx = self.services.store().begin().await?;
            let Some(link) = lock_link(&mut tx, id).await? else {
                return Ok(());
            };
            match link.status() {
                LinkStatus::Active => {
                    request_cancel(&mut tx, &link, source, actor, now).await?;
                }
                LinkStatus::Pending => {
                    close(
                        &mut tx,
                        &link,
                        LinkStatus::Cancelled,
                        Some(source),
                        actor,
                        None,
                        now,
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(());
                }
                LinkStatus::Cancelling => {}
                LinkStatus::Cancelled | LinkStatus::Failed => return Ok(()),
            }
            tx.commit().await?;
            let mut conn = self.services.store().conn().await?;
            link_by_id(&mut conn, id).await?.unwrap_or(link)
        };
        self.push_cancel(link, actor).await
    }

    async fn push_cancel(&self, link: Link, actor: Actor) -> ServiceResult<()> {
        let now = self.services.now();
        let Some(provider) = self.provider(&link.provider) else {
            return Err(ServiceError::NotFound("платёжная система"));
        };
        let config = self.services.payments().config(&link.provider);
        let title = title_of(self.services, &link.provider);
        let blind = link.email.trim().is_empty();
        let answer = if blind {
            match provider.fetch_renewal(&config, &link.external_id).await {
                Ok(state)
                    if matches!(
                        state.status,
                        RenewalStatus::Cancelled | RenewalStatus::Failed
                    ) =>
                {
                    Ok(RenewalCancel::Cancelled)
                }
                _ => Err(PaymentError::Rejected(no_email(&title, Locale::Ru))),
            }
        } else {
            provider
                .cancel_renewal(&config, &link.external_id, &link.email)
                .await
        };
        let mut tx = self.services.store().begin().await?;
        let Some(current) = lock_link(&mut tx, link.id).await? else {
            return Ok(());
        };
        if current.status() != LinkStatus::Cancelling {
            return Ok(());
        }
        match answer {
            Ok(RenewalCancel::Cancelled | RenewalCancel::Missing) => {
                let source = current.cancel_source.as_deref().and_then(source_of);
                close(
                    &mut tx,
                    &current,
                    LinkStatus::Cancelled,
                    source,
                    actor,
                    None,
                    now,
                )
                .await?;
            }
            Err(err) => {
                let attempts = current.cancel_attempts + 1;
                let wait = CANCEL_RETRY_MINUTES[usize::try_from(attempts - 1)
                    .unwrap_or(0)
                    .min(CANCEL_RETRY_MINUTES.len() - 1)];
                let text = err.to_string();
                sqlx::query(
                    "UPDATE provider_subscriptions SET cancel_attempts = $2, cancel_error = $3, \
                     check_at = $4, updated_at = now() WHERE id = $1",
                )
                .bind(current.id)
                .bind(attempts)
                .bind(&text)
                .bind(now + Duration::minutes(wait))
                .execute(tx.conn())
                .await?;
                if !refusal_is_transient(&err) {
                    notice(
                        &mut tx,
                        &current,
                        format!("renewal_cancel_refused:{}", current.external_id),
                        NoticeKind::ManualCredit,
                        None,
                        if blind {
                            format!(
                                "Панель не может отменить автопродление клиента в {title}: почта \
                                 покупателя неизвестна. Списания идут, пока подписка не отменена: \
                                 отмените её в кабинете {title}, панель увидит отмену при сверке"
                            )
                        } else {
                            format!(
                                "{title} не отменила автопродление клиента: {text}. Панель повторяет \
                                 запрос, но списания идут, пока подписка не отменена: отмените её в \
                                 кабинете {title}"
                            )
                        },
                    )
                    .await?;
                }
                tracing::warn!(link = current.id, error = %text, "автопродление не отменено в платёжной системе, повторим");
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn check_subscription(
        &self,
        subscription: SubscriptionId,
    ) -> ServiceResult<Option<RenewalView>> {
        let link = {
            let mut conn = self.services.store().conn().await?;
            live_link(&mut conn, subscription).await?
        };
        if let Some(link) = link {
            match link.status() {
                LinkStatus::Cancelling => self.push_cancel(link, Actor::System).await?,
                _ => self.reconcile(link).await?,
            }
        }
        self.view(subscription).await
    }

    pub async fn sweep(&self, now: DateTime<Utc>) -> ServiceResult<SweepReport> {
        let mut report = SweepReport::default();
        match self.adopt_legacy(now).await {
            Ok(adoption) => {
                report.adopted = adoption.adopted;
                report.legacy_checked = adoption.checked;
                report.legacy_skipped = adoption.skipped;
            }
            Err(err) => {
                tracing::warn!(error = %err, "договоры Lava.top старой панели не проверены");
            }
        }
        let due: Vec<Link> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_as(&format!(
                "SELECT {LINK_COLUMNS} FROM provider_subscriptions \
                 WHERE status IN ('active', 'cancelling') AND check_at IS NOT NULL AND check_at <= $1 \
                 ORDER BY check_at LIMIT $2"
            ))
            .bind(now)
            .bind(SWEEP_BATCH)
            .fetch_all(conn.conn())
            .await?
        };
        for link in due {
            report.checked += 1;
            let id = link.id;
            let cancelling = link.status() == LinkStatus::Cancelling;
            let result = if cancelling {
                self.push_cancel(link, Actor::System).await.map(|_| 0)
            } else {
                self.reconcile_counted(link).await
            };
            match result {
                Ok(found) => report.charges += found,
                Err(err) => {
                    report.failed += 1;
                    tracing::warn!(link = id, error = %err, "сверка автопродления не выполнена");
                }
            }
            if cancelling {
                let mut conn = self.services.store().conn().await?;
                if link_by_id(&mut conn, id)
                    .await?
                    .is_some_and(|l| l.status() == LinkStatus::Cancelled)
                {
                    report.cancelled += 1;
                }
            }
        }
        Ok(report)
    }

    async fn reconcile(&self, link: Link) -> ServiceResult<()> {
        self.reconcile_counted(link).await.map(|_| ())
    }

    async fn reconcile_counted(&self, link: Link) -> ServiceResult<usize> {
        let now = self.services.now();
        let Some(provider) = self.provider(&link.provider) else {
            return Ok(0);
        };
        let config = self.services.payments().config(&link.provider);
        let state = match provider.fetch_renewal(&config, &link.external_id).await {
            Ok(state) => state,
            Err(err) => {
                let wait = if err.is_retryable() {
                    1
                } else {
                    OVERDUE_RETRY_HOURS
                };
                let mut conn = self.services.store().conn().await?;
                sqlx::query(
                    "UPDATE provider_subscriptions SET check_at = $2, updated_at = now() WHERE id = $1",
                )
                .bind(link.id)
                .bind(now + Duration::hours(wait))
                .execute(conn.conn())
                .await?;
                return Err(err.into());
            }
        };
        let mut found = 0usize;
        for charge in state.charges.iter().filter(|c| c.paid) {
            if charge.id.eq_ignore_ascii_case(&link.external_id) {
                continue;
            }
            if link
                .charges_since
                .is_some_and(|since| charge.at.is_none_or(|at| at <= since))
            {
                continue;
            }
            let known = {
                let mut conn = self.services.store().conn().await?;
                conn.payments()
                    .by_provider_txid(&link.provider, &charge.id)
                    .await?
                    .is_some()
                    || charge_known(&mut conn, link.id, &charge.id).await?
            };
            if known {
                continue;
            }
            match provider
                .confirm_renewal(&config, &link.external_id, &charge.id)
                .await
            {
                Ok(Some(outcome)) => {
                    self.services
                        .inbox()
                        .apply_outcome(&link.provider, &outcome)
                        .await?;
                    found += 1;
                    let mut conn = self.services.store().conn().await?;
                    audit(
                        &mut conn,
                        Actor::System,
                        "subscription.renewal_charge_found",
                        &link,
                        serde_json::json!({ "charge": charge.id, "at": charge.at }),
                    )
                    .await?;
                }
                Ok(None) => {
                    let mut conn = self.services.store().conn().await?;
                    notice(
                        &mut conn,
                        &link,
                        format!("renewal_unknown:{}", charge.id),
                        NoticeKind::ManualCredit,
                        None,
                        format!(
                            "{} показывает списание №{} по автопродлению клиента, но не подтверждает \
                             его как продление этой подписки. Сверьте платёж в кабинете {} и зачислите вручную, если деньги списаны",
                            title_of(self.services, &link.provider),
                            charge.id,
                            title_of(self.services, &link.provider)
                        ),
                    )
                    .await?;
                }
                Err(err) => return Err(err.into()),
            }
        }
        let mut tx = self.services.store().begin().await?;
        let Some(current) = lock_link(&mut tx, link.id).await? else {
            return Ok(found);
        };
        match state.status {
            RenewalStatus::Cancelled | RenewalStatus::Failed if current.status().live() => {
                let status = if state.status == RenewalStatus::Failed {
                    LinkStatus::Failed
                } else {
                    LinkStatus::Cancelled
                };
                close(
                    &mut tx,
                    &current,
                    status,
                    Some(CancelSource::Provider),
                    Actor::System,
                    state.paid_until,
                    now,
                )
                .await?;
            }
            _ => {
                let next = match (state.status, state.paid_until, current.next_charge_at) {
                    (RenewalStatus::Active, Some(until), _)
                        if until > now - Duration::days(400) =>
                    {
                        until
                    }
                    (_, _, Some(next)) => next,
                    _ => current.cycle(self.services).after(now),
                };
                sqlx::query(
                    "UPDATE provider_subscriptions SET checked_at = $2, check_at = $3, \
                     next_charge_at = $4, paid_until = COALESCE($5, paid_until), updated_at = now() WHERE id = $1",
                )
                .bind(current.id)
                .bind(now)
                .bind(next_check(next, now))
                .bind(next)
                .bind(state.paid_until)
                .execute(tx.conn())
                .await?;
            }
        }
        tx.commit().await?;
        Ok(found)
    }
}

async fn charge_known(db: &mut impl Db, link: i64, external: &str) -> ServiceResult<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_subscription_charges \
         WHERE provider_subscription_id = $1 AND lower(external_id) = lower($2)",
    )
    .bind(link)
    .bind(external)
    .fetch_one(db.conn())
    .await?
        > 0)
}

fn refusal_is_transient(err: &PaymentError) -> bool {
    err.is_retryable() || matches!(err, PaymentError::Http { status: 408 | 429 })
}

pub(crate) fn quick_repeat(last: Option<DateTime<Utc>>, at: DateTime<Utc>) -> bool {
    last.is_some_and(|last| {
        (at - last).num_seconds().abs() < Duration::days(QUICK_REPEAT_DAYS).num_seconds()
    })
}

pub(crate) fn devices_refusal() -> ServiceError {
    ServiceError::rejected(
        "renewal.devices",
        "Пока у подписки включено автопродление через Lava.top, доп. устройства не докупить: \
         отключите автопродление или выберите тариф с нужным числом устройств",
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundTerm {
    pub provider: String,
    pub title: String,
    pub tariff_id: i64,
    pub period_id: i64,
    pub amount: Money,
    pub every: String,
}

pub fn bound_terms(services: &Services) -> Vec<BoundTerm> {
    let registry = services.payments();
    registry
        .providers()
        .iter()
        .flat_map(|provider| {
            let config = registry.config(provider.id());
            if !provider.renewals_enabled(&config) {
                return Vec::new();
            }
            provider
                .renewal_plans(&config)
                .into_iter()
                .map(|plan| BoundTerm {
                    provider: provider.id().to_string(),
                    title: provider.title().to_string(),
                    tariff_id: plan.binding.tariff_id,
                    period_id: plan.binding.period_id,
                    amount: plan.amount,
                    every: plan.every.to_string(),
                })
                .collect()
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RenewalStats {
    pub active: i64,
    pub cancelling: i64,
    pub next_charge_at: Option<DateTime<Utc>>,
    pub old_price: i64,
}

type StatsRow = (
    String,
    Option<i64>,
    Option<i64>,
    i64,
    String,
    Option<DateTime<Utc>>,
);

impl Renewals<'_> {
    pub async fn stats(&self, provider: &str) -> ServiceResult<RenewalStats> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<StatsRow> = sqlx::query_as(
            "SELECT status, tariff_id, period_id, amount_minor, currency, next_charge_at \
             FROM provider_subscriptions WHERE provider = $1 AND status IN ('active', 'cancelling')",
        )
        .bind(provider)
        .fetch_all(conn.conn())
        .await?;
        let tariffs = conn.tariffs().list(true).await?;
        let mut stats = RenewalStats::default();
        for (status, tariff, period, amount, currency, next) in rows {
            if status == "active" {
                stats.active += 1;
                if let Some(next) = next {
                    stats.next_charge_at = Some(
                        stats
                            .next_charge_at
                            .map_or(next, |at: DateTime<Utc>| at.min(next)),
                    );
                }
            } else {
                stats.cancelling += 1;
            }
            let price = tariffs
                .iter()
                .find(|t| Some(t.id.0) == tariff)
                .and_then(|t| t.periods.iter().find(|p| Some(p.id.0) == period))
                .and_then(|p| Currency::parse(&currency).ok().and_then(|c| p.price(c)))
                .map(|m| m.minor);
            if price != Some(amount) {
                stats.old_price += 1;
            }
        }
        Ok(stats)
    }
}

pub(crate) async fn check_plans(
    db: &mut impl Db,
    provider: &dyn PaymentProvider,
    config: &sn_payments::ProviderConfig,
) -> ServiceResult<()> {
    for plan in provider.renewal_plans(config) {
        let refused = |message: String| ServiceError::rejected("settings.renewal_plan", message);
        let tariff = match db
            .tariffs()
            .get(sn_domain::ids::TariffId(plan.binding.tariff_id))
            .await
        {
            Ok(tariff) => tariff,
            Err(sn_store::StoreError::NotFound(_)) => {
                return Err(refused(format!(
                    "Подписка {} «{}» привязана к удалённому тарифу: выберите другой срок",
                    provider.title(),
                    plan.name
                )));
            }
            Err(err) => return Err(err.into()),
        };
        let Some(period) = tariff.period(PeriodId(plan.binding.period_id)) else {
            return Err(refused(format!(
                "Подписка {} «{}» привязана к сроку, которого больше нет у тарифа «{}»",
                provider.title(),
                plan.name,
                tariff.name
            )));
        };
        if !(plan.min_days..=plan.max_days).contains(&period.days) {
            return Err(refused(format!(
                "Подписка {} «{}» списывает {}, а срок тарифа «{}» — {} дн.: выберите срок подходящей длины",
                provider.title(),
                plan.name,
                plan.every,
                tariff.name,
                period.days
            )));
        }
    }
    Ok(())
}
