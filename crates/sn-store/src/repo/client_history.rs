use super::audit::{AuditRecord, AuditRepo, audit_entity};
use super::clients::{ClientsRepo, display_name};
use super::events::{EventsRepo, StoredEvent};
use super::orders::OrdersRepo;
use super::payments::{PaymentsRepo, charge_review_filter};
use super::referrals::referral_audit;
use super::subscriptions::PAID_LIKE;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::ids::{ClientId, EventId, OrderId, PaymentId};
use sn_domain::money::Currency;
use sn_domain::order::Order;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::Payment;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "type", content = "record", rename_all = "snake_case")]
pub enum ClientHistoryEntry {
    Payment(Payment),
    Order(Box<Order>),
    Event(StoredEvent),
    Audit(AuditRecord),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ClientHistoryItem {
    pub at: DateTime<Utc>,
    pub entry: ClientHistoryEntry,
}

const SOURCE_AUDIT: i32 = 1;
const SOURCE_EVENT: i32 = 2;
const SOURCE_PAYMENT: i32 = 3;
const SOURCE_ORDER: i32 = 4;

const REPEATED_EVENTS: &str = "('payment_paid', 'payment_refunded', 'order_fulfilled', \
     'subscription_created', 'subscription_renewed', 'tariff_changed', \
     'client_blocked', 'client_unblocked')";

fn feed_sql(money: bool) -> String {
    let payment_audit = if money {
        format!(
            "UNION ALL SELECT '{}', id FROM payments WHERE client_id = $1",
            audit_entity::PAYMENT
        )
    } else {
        String::new()
    };
    let balance_events = if money {
        "AND NOT (e.kind = 'balance_changed' \
         AND COALESCE(e.payload ->> 'kind', '') IN ('top_up', 'purchase', 'auto_renew'))"
    } else {
        "AND e.kind <> 'balance_changed'"
    };
    let payments = if money {
        format!(
            "UNION ALL SELECT {SOURCE_PAYMENT}, p.id, COALESCE(p.paid_at, p.created_at) FROM payments p WHERE p.client_id = $1 \
             AND (p.status IN ('paid', 'partially_refunded', 'refunded', 'failed') OR ({}))",
            charge_review_filter("p")
        )
    } else {
        String::new()
    };
    format!(
        "SELECT {SOURCE_AUDIT} AS src, a.id, a.created_at AS at FROM audit_log a \
         WHERE a.id IN ( \
             SELECT x.id FROM audit_log x WHERE (x.entity_kind, x.entity_id) IN ( \
                 SELECT '{client}', $1::bigint \
                 UNION ALL SELECT '{subscription}', id FROM subscriptions WHERE client_id = $1 \
                 UNION ALL SELECT '{order}', id FROM orders WHERE client_id = $1 \
                 {payment_audit} \
                 UNION ALL SELECT '{ticket}', id FROM tickets WHERE client_id = $1) \
             UNION ALL SELECT x.id FROM audit_log x WHERE x.actor_kind = 'client' AND x.actor_id = $1 \
             UNION ALL SELECT x.id FROM audit_log x WHERE x.action IN ('{referrer_set}', '{referrer_removed}') \
                 AND $1::bigint::text IN (x.details ->> 'referrer_id', x.details ->> 'previous_referrer_id')) \
         UNION ALL SELECT {SOURCE_EVENT}, e.id, e.occurred_at FROM domain_events e WHERE e.client_id = $1 \
             AND e.kind NOT IN {REPEATED_EVENTS} \
             AND NOT (e.kind = 'subscription_status_changed' \
                 AND COALESCE(e.payload ->> 'to', '') NOT IN ('expired', 'limited')) \
             {balance_events} \
         {payments} \
         UNION ALL SELECT {SOURCE_ORDER}, o.id, o.created_at FROM orders o WHERE o.client_id = $1 \
             AND o.status IN ('paid', 'fulfilled', 'review', 'refunded') \
             AND NOT EXISTS (SELECT 1 FROM payments x WHERE x.order_id = o.id)",
        client = audit_entity::CLIENT,
        subscription = audit_entity::SUBSCRIPTION,
        order = audit_entity::ORDER,
        ticket = audit_entity::TICKET,
        referrer_set = referral_audit::REFERRER_SET,
        referrer_removed = referral_audit::REFERRER_REMOVED,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClientPerson {
    pub client_id: ClientId,
    pub name: Option<String>,
    pub telegram_username: Option<String>,
    pub telegram_id: Option<String>,
    pub email: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PersonRow {
    id: i64,
    telegram_id: Option<String>,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClientCardStats {
    pub paid_total_minor: i64,
    pub payments_count: i64,
    pub referrals_count: i64,
    pub referrer: Option<ClientPerson>,
    pub referrer_manual: bool,
    pub cabinet_seen_at: Option<DateTime<Utc>>,
    pub miniapp_seen_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct StatsRow {
    paid_total_minor: i64,
    payments_count: i64,
    referrals_count: i64,
    referrer_id: Option<i64>,
    referrer_manual: Option<bool>,
    referrer_username: Option<String>,
    referrer_first_name: Option<String>,
    referrer_last_name: Option<String>,
    cabinet_seen_at: Option<DateTime<Utc>>,
    miniapp_seen_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct FeedRow {
    src: i32,
    id: i64,
    at: DateTime<Utc>,
    more: bool,
}

impl<'c> ClientsRepo<'c> {
    pub async fn history(
        &mut self,
        id: ClientId,
        page: PageRequest,
        money: bool,
    ) -> StoreResult<Page<ClientHistoryItem>> {
        let before = page
            .after
            .map(|micros| {
                DateTime::<Utc>::from_timestamp_micros(micros)
                    .ok_or_else(|| StoreError::Invalid("неверная позиция в истории".into()))
            })
            .transpose()?;
        let rows: Vec<FeedRow> = sqlx::query_as(&format!(
            "WITH feed AS ({feed}), \
             page AS (SELECT src, id, at FROM feed WHERE $2::timestamptz IS NULL OR at < $2), \
             bound AS (SELECT at FROM page ORDER BY at DESC OFFSET $3 LIMIT 1) \
             SELECT p.src, p.id, p.at, EXISTS (SELECT 1 FROM page q, bound b WHERE q.at < b.at) AS more \
             FROM page p WHERE p.at >= COALESCE((SELECT at FROM bound), '-infinity'::timestamptz) \
             ORDER BY p.at DESC, p.src, p.id DESC",
            feed = feed_sql(money)
        ))
        .bind(id.0)
        .bind(before)
        .bind(i64::from(page.limit.max(1)) - 1)
        .fetch_all(&mut *self.conn)
        .await?;
        let ids = |source: i32| -> Vec<i64> {
            rows.iter()
                .filter(|row| row.src == source)
                .map(|row| row.id)
                .collect()
        };
        let payment_ids: Vec<PaymentId> = ids(SOURCE_PAYMENT).into_iter().map(PaymentId).collect();
        let order_ids: Vec<OrderId> = ids(SOURCE_ORDER).into_iter().map(OrderId).collect();
        let event_ids: Vec<EventId> = ids(SOURCE_EVENT).into_iter().map(EventId).collect();
        let audit_ids = ids(SOURCE_AUDIT);
        let mut payments: HashMap<i64, Payment> = PaymentsRepo::new(&mut *self.conn)
            .by_ids(&payment_ids)
            .await?
            .into_iter()
            .map(|p| (p.id.0, p))
            .collect();
        let mut orders: HashMap<i64, Order> = OrdersRepo::new(&mut *self.conn)
            .by_ids(&order_ids)
            .await?
            .into_iter()
            .map(|o| (o.id.0, o))
            .collect();
        let mut events: HashMap<i64, StoredEvent> = EventsRepo::new(&mut *self.conn)
            .by_ids(&event_ids)
            .await?
            .into_iter()
            .map(|e| (e.id.0, e))
            .collect();
        let mut audit: HashMap<i64, AuditRecord> = AuditRepo::new(&mut *self.conn)
            .by_ids(&audit_ids)
            .await?
            .into_iter()
            .map(|a| (a.id, a))
            .collect();
        let more = rows.first().is_some_and(|row| row.more);
        let next = if more {
            rows.last().map(|row| row.at.timestamp_micros())
        } else {
            None
        };
        let items = rows
            .iter()
            .filter_map(|row| {
                let entry = match row.src {
                    SOURCE_PAYMENT => payments.remove(&row.id).map(ClientHistoryEntry::Payment),
                    SOURCE_ORDER => orders
                        .remove(&row.id)
                        .map(|order| ClientHistoryEntry::Order(Box::new(order))),
                    SOURCE_EVENT => events.remove(&row.id).map(ClientHistoryEntry::Event),
                    _ => audit.remove(&row.id).map(ClientHistoryEntry::Audit),
                }?;
                Some(ClientHistoryItem { at: row.at, entry })
            })
            .collect();
        Ok(Page { items, next })
    }

    pub async fn card_stats(
        &mut self,
        id: ClientId,
        currency: Currency,
    ) -> StoreResult<ClientCardStats> {
        let row: StatsRow = sqlx::query_as(&format!(
            "SELECT \
             COALESCE((SELECT sum(p.amount_minor - p.refunded_minor) FROM payments p WHERE p.client_id = $1 \
                 AND p.currency = $2 AND p.status IN {PAID_LIKE} AND p.provider <> 'balance'), 0)::bigint AS paid_total_minor, \
             (SELECT count(*) FROM payments p WHERE p.client_id = $1 \
                 AND p.currency = $2 AND p.status IN {PAID_LIKE} AND p.provider <> 'balance') AS payments_count, \
             (SELECT count(*) FROM referral_links r WHERE r.referrer_id = $1) AS referrals_count, \
             ref.referrer_id, ref.manual AS referrer_manual, ref.meta ->> 'username' AS referrer_username, \
             ref.meta ->> 'first_name' AS referrer_first_name, ref.meta ->> 'last_name' AS referrer_last_name, \
             (SELECT max(s.last_seen_at) FROM client_sessions s WHERE s.client_id = $1 AND s.channel = 'cabinet') AS cabinet_seen_at, \
             (SELECT max(s.last_seen_at) FROM client_sessions s WHERE s.client_id = $1 AND s.channel = 'miniapp') AS miniapp_seen_at \
             FROM (SELECT 1) one \
             LEFT JOIN LATERAL (SELECT r.referrer_id, r.set_at IS NOT NULL AS manual, i.meta FROM referral_links r \
                 LEFT JOIN LATERAL (SELECT meta FROM client_identities WHERE client_id = r.referrer_id \
                     AND kind = 'telegram' ORDER BY id LIMIT 1) i ON true \
                 WHERE r.client_id = $1) ref ON true"
        ))
        .bind(id.0)
        .bind(currency.code())
        .fetch_one(&mut *self.conn)
        .await?;
        Ok(ClientCardStats {
            paid_total_minor: row.paid_total_minor,
            payments_count: row.payments_count,
            referrals_count: row.referrals_count,
            referrer: row.referrer_id.map(|referrer| ClientPerson {
                client_id: ClientId(referrer),
                name: display_name(
                    row.referrer_first_name.as_deref(),
                    row.referrer_last_name.as_deref(),
                ),
                telegram_username: row.referrer_username,
                telegram_id: None,
                email: None,
            }),
            referrer_manual: row.referrer_manual.unwrap_or(false),
            cabinet_seen_at: row.cabinet_seen_at,
            miniapp_seen_at: row.miniapp_seen_at,
        })
    }

    pub async fn people(
        &mut self,
        ids: &[ClientId],
    ) -> StoreResult<HashMap<ClientId, ClientPerson>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<i64> = ids.iter().map(|id| id.0).collect();
        let rows: Vec<PersonRow> = sqlx::query_as(
            "SELECT c.id, tg.value AS telegram_id, tg.meta ->> 'username' AS username, \
             tg.meta ->> 'first_name' AS first_name, tg.meta ->> 'last_name' AS last_name, \
             em.value AS email FROM clients c \
             LEFT JOIN LATERAL (SELECT i.value, i.meta FROM client_identities i WHERE i.client_id = c.id \
                 AND i.kind = 'telegram' ORDER BY i.id LIMIT 1) tg ON true \
             LEFT JOIN LATERAL (SELECT i.value FROM client_identities i WHERE i.client_id = c.id \
                 AND i.kind = 'email' ORDER BY i.id LIMIT 1) em ON true \
             WHERE c.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    ClientId(row.id),
                    ClientPerson {
                        client_id: ClientId(row.id),
                        name: display_name(row.first_name.as_deref(), row.last_name.as_deref()),
                        telegram_username: row.username,
                        telegram_id: row.telegram_id,
                        email: row.email,
                    },
                )
            })
            .collect())
    }

    pub async fn history_count(&mut self, id: ClientId, money: bool) -> StoreResult<i64> {
        Ok(
            sqlx::query_scalar(&format!("SELECT count(*) FROM ({}) f", feed_sql(money)))
                .bind(id.0)
                .fetch_one(&mut *self.conn)
                .await?,
        )
    }
}
