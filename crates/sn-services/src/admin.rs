use crate::Actor;
use crate::clients::{OrderBrief, PaymentBrief, TicketBrief, admin_event, order_brief};
use crate::error::{ServiceError, ServiceResult};
use crate::inbox::{AcceptResult, RefundCommand, RefundResult};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::events::DomainEvent;
use sn_domain::ids::{AdminId, ClientId, OrderId, PaymentId, TariffId};
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::order::OrderStatus;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{
    ADMIN_CANCELED_REASON, AUTORENEW_METHOD, Payment, PaymentStatus, ProviderId, RefundMethod,
    RefundSubscriptionAction, providers,
};
use sn_payments::{PaymentRef, ProviderStatus, WebhookOutcome, WebhookReply};
use sn_remnawave::NodeState;
use sn_store::ClientPerson;
use sn_store::{Db, StoreError};
use std::collections::HashMap;
use uuid::Uuid;

pub const FEED_MAX: u32 = 100;
const MONEY_EVENTS: [&str; 3] = ["payment_paid", "payment_refunded", "balance_changed"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DashboardCounts {
    pub active_paid: i64,
    pub trials: i64,
    pub expiring_soon: i64,
    pub never_connected: i64,
    pub limited: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DashboardRevenue {
    pub today: Money,
    pub yesterday: Money,
    pub week: Money,
    pub month: Money,
    pub today_stars: i64,
    pub by_day: Vec<DayAmount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DayAmount {
    pub day: chrono::NaiveDate,
    pub amount: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sees {
    pub money: bool,
    pub refunds: bool,
    pub infrastructure: bool,
}

impl Sees {
    pub const ALL: Sees = Sees {
        money: true,
        refunds: true,
        infrastructure: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DashboardToday {
    pub payments: Option<i64>,
    pub new_clients: i64,
    pub new_clients_paid: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeBrief {
    pub uuid: Uuid,
    pub name: String,
    pub country_code: String,
    pub state: NodeState,
    pub users_online: i64,
    pub traffic_alert: bool,
    pub traffic_used_bytes: Option<i64>,
    pub traffic_limit_bytes: Option<i64>,
    pub traffic_reset_day: Option<i64>,
    pub last_status_change: Option<DateTime<Utc>>,
    pub last_status_message: Option<String>,
    pub down_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemnaOverview {
    pub online_now: i64,
    pub users_total: i64,
    pub status_counts: std::collections::BTreeMap<String, i64>,
    pub nodes_total: usize,
    pub nodes_online: usize,
    pub nodes: Vec<NodeBrief>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardView {
    pub currency: Currency,
    pub subscriptions: Option<DashboardCounts>,
    pub today: Option<DashboardToday>,
    pub revenue: Option<DashboardRevenue>,
    pub stats_error: Option<String>,
    pub support_waiting: i64,
    pub orders_review: Option<i64>,
    pub stars_review: Option<i64>,
    pub refund_actions_failed: Option<i64>,
    pub charges_review: Option<i64>,
    pub webhooks: Option<crate::inbox::WebhookAttention>,
    pub remna: Option<RemnaOverview>,
    pub remna_error: Option<String>,
    pub server: Option<crate::server_load::ServerLoad>,
    pub setup: SetupState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SetupState {
    pub remnawave: bool,
    pub bot: bool,
    pub tariffs: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    #[default]
    All,
    Nodes,
    Clients,
    Payments,
    Support,
    Highlights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct FeedQuery {
    #[serde(default)]
    pub kind: FeedKind,
    #[serde(default)]
    pub remna_before: Option<i64>,
    #[serde(default)]
    pub core_before: Option<i64>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedSource {
    Remnawave,
    Core,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeedItem {
    pub source: FeedSource,
    pub id: i64,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub title: String,
    pub client_id: Option<ClientId>,
    pub subject: Option<String>,
    pub data: Value,
    pub client: Option<ClientPerson>,
    pub order: Option<OrderBrief>,
    pub payment: Option<PaymentBrief>,
    pub ticket: Option<TicketBrief>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaymentListItem {
    #[serde(flatten)]
    pub payment: Payment,
    pub client: Option<ClientPerson>,
    pub order: Option<OrderBrief>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FeedPage {
    pub items: Vec<FeedItem>,
    pub remna_before: Option<i64>,
    pub core_before: Option<i64>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PaymentFilter {
    #[serde(default)]
    pub status: Option<PaymentStatus>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub client_id: Option<ClientId>,
    #[serde(default)]
    pub id: Option<PaymentId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MarkPaidRequest {
    #[serde(default)]
    pub provider_txid: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CancelPaymentRequest {
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelPaymentResult {
    pub order_canceled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum MarkPaidResult {
    Accepted { order_id: OrderId },
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RefundRequest {
    pub amount_minor: Option<i64>,
    pub method: RefundMethod,
    pub subscription_action: RefundSubscriptionAction,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub key: Option<Uuid>,
    #[serde(default)]
    pub outside: bool,
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

impl TryFrom<PaymentRow> for Payment {
    type Error = ServiceError;

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

#[derive(sqlx::FromRow)]
struct RemnaFeedRow {
    id: i64,
    scope: String,
    event: String,
    occurred_at: DateTime<Utc>,
    subject_id: Option<String>,
    payload: Value,
}

#[derive(sqlx::FromRow)]
struct CoreFeedRow {
    id: i64,
    occurred_at: DateTime<Utc>,
    client_id: Option<i64>,
    payload: sqlx::types::Json<Value>,
}

fn text(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    match current {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn pick(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(key).map(|v| (key.to_string(), v.clone())))
            .collect(),
    )
}

pub fn remna_feed_entry(scope: &str, event: &str, payload: &Value) -> (String, Value) {
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    match scope {
        "node" => {
            let name = text(&data, &["name"]).unwrap_or_else(|| "?".into());
            let title = match event {
                "node.connection_lost" => format!("Нода {name} не отвечает"),
                "node.connection_restored" => format!("Нода {name} снова в сети"),
                "node.traffic_notify" => format!("Нода {name}: трафик подходит к лимиту"),
                "node.created" => format!("Добавлена нода {name}"),
                "node.deleted" => format!("Нода {name} удалена"),
                "node.disabled" => format!("Нода {name} выключена"),
                "node.enabled" => format!("Нода {name} включена"),
                "node.modified" => format!("Нода {name} изменена"),
                other => format!("Нода {name}: {other}"),
            };
            (
                title,
                pick(
                    &data,
                    &[
                        "uuid",
                        "name",
                        "countryCode",
                        "isConnected",
                        "isDisabled",
                        "lastStatusChange",
                        "lastStatusMessage",
                        "usersOnline",
                        "trafficUsedBytes",
                        "trafficLimitBytes",
                    ],
                ),
            )
        }
        "user" | "user_hwid_devices" => {
            let user = data.get("user").cloned().unwrap_or(Value::Null);
            let name = text(&user, &["username"]).unwrap_or_else(|| "?".into());
            let title = match event {
                "user.created" => format!("Создан пользователь {name}"),
                "user.expired" => format!("Подписка {name} истекла"),
                "user.expires_in_72_hours"
                | "user.expires_in_48_hours"
                | "user.expires_in_24_hours"
                | "user.expiration" => {
                    format!("Подписка {name} скоро истекает")
                }
                "user.limited" => format!("У {name} закончился трафик"),
                "user.traffic_reset" => format!("У {name} сброшен трафик"),
                "user.first_connected" => format!("{name} подключился впервые"),
                "user.not_connected" => format!("{name} так и не подключился"),
                "user.disabled" => format!("Пользователь {name} отключён"),
                "user.enabled" => format!("Пользователь {name} включён"),
                "user.revoked" => format!("У {name} перевыпущена ссылка"),
                "user_hwid_devices.added" => format!("{name}: новое устройство"),
                "user_hwid_devices.deleted" => format!("{name}: устройство отключено"),
                other => format!("{name}: {other}"),
            };
            let mut summary = pick(&user, &["id", "username", "status", "expireAt"]);
            if let (Some(device), Value::Object(map)) = (data.get("hwidUserDevice"), &mut summary) {
                map.insert(
                    "device".into(),
                    pick(
                        device,
                        &["platform", "osVersion", "deviceModel", "createdAt"],
                    ),
                );
            }
            (title, summary)
        }
        "crm" => {
            let node = text(&data, &["nodeName"]).unwrap_or_else(|| "?".into());
            let provider = text(&data, &["providerName"]).unwrap_or_default();
            let title = if event.contains("overdue") {
                format!("Оплата сервера {node} ({provider}) просрочена")
            } else {
                format!("Скоро оплата сервера {node} ({provider})")
            };
            (
                title,
                pick(
                    &data,
                    &["nodeName", "providerName", "nextBillingAt", "loginUrl"],
                ),
            )
        }
        _ => (format!("Remnawave: {event}"), Value::Null),
    }
}

pub fn core_feed_title(event: &DomainEvent) -> String {
    match event {
        DomainEvent::ClientRegistered { .. } => "Новый клиент".into(),
        DomainEvent::ClientBlocked { .. } => "Клиент заблокирован".into(),
        DomainEvent::ClientUnblocked { .. } => "Клиент разблокирован".into(),
        DomainEvent::PaymentPaid { amount, .. } => format!("Оплата {amount}"),
        DomainEvent::PaymentRefunded { amount, .. } => format!("Возврат {amount}"),
        DomainEvent::OrderFulfilled { .. } => "Заказ выдан".into(),
        DomainEvent::SubscriptionCreated { .. } => "Новая подписка".into(),
        DomainEvent::SubscriptionRenewed { .. } => "Подписка продлена".into(),
        DomainEvent::TariffChanged { .. } => "Смена тарифа".into(),
        DomainEvent::SubscriptionStatusChanged { to, .. } => {
            format!("Статус подписки: {}", to.as_str())
        }
        DomainEvent::BalanceChanged { delta, .. } => format!("Баланс {delta}"),
        DomainEvent::TicketOpened { .. } => "Новое обращение в поддержку".into(),
        DomainEvent::TicketAnswered { .. } => "Ответ в поддержке".into(),
    }
}

fn remna_scopes(kind: FeedKind, infrastructure: bool) -> Option<Vec<String>> {
    let people = || vec!["user".into(), "user_hwid_devices".into()];
    match kind {
        FeedKind::All if infrastructure => None,
        FeedKind::All | FeedKind::Clients => Some(people()),
        FeedKind::Nodes | FeedKind::Highlights if infrastructure => {
            Some(vec!["node".into(), "crm".into()])
        }
        FeedKind::Nodes | FeedKind::Highlights | FeedKind::Payments | FeedKind::Support => {
            Some(Vec::new())
        }
    }
}

fn core_kinds(kind: FeedKind, include_money: bool) -> Option<Vec<String>> {
    let all = [
        "client_registered",
        "client_blocked",
        "client_unblocked",
        "payment_paid",
        "payment_refunded",
        "order_fulfilled",
        "subscription_created",
        "subscription_renewed",
        "tariff_changed",
        "subscription_status_changed",
        "balance_changed",
        "ticket_opened",
        "ticket_answered",
    ];
    let allowed = |k: &&&str| include_money || !MONEY_EVENTS.contains(*k);
    match kind {
        FeedKind::All if include_money => None,
        FeedKind::All => Some(all.iter().filter(allowed).map(|k| k.to_string()).collect()),
        FeedKind::Nodes => Some(Vec::new()),
        FeedKind::Clients => Some(
            [
                "client_registered",
                "client_blocked",
                "client_unblocked",
                "subscription_created",
                "subscription_renewed",
                "tariff_changed",
                "subscription_status_changed",
                "ticket_opened",
                "ticket_answered",
            ]
            .iter()
            .map(|k| k.to_string())
            .collect(),
        ),
        FeedKind::Payments => Some(
            [
                "payment_paid",
                "payment_refunded",
                "balance_changed",
                "order_fulfilled",
            ]
            .iter()
            .filter(allowed)
            .map(|k| k.to_string())
            .collect(),
        ),
        FeedKind::Support => Some(vec!["ticket_opened".into(), "ticket_answered".into()]),
        FeedKind::Highlights => Some(
            [
                "client_registered",
                "client_blocked",
                "payment_paid",
                "payment_refunded",
                "ticket_opened",
            ]
            .iter()
            .filter(allowed)
            .map(|k| k.to_string())
            .collect(),
        ),
    }
}

pub fn merge_feed(remna: Vec<FeedItem>, core: Vec<FeedItem>, limit: usize) -> Vec<FeedItem> {
    let mut remna = remna.into_iter().peekable();
    let mut core = core.into_iter().peekable();
    let mut out = Vec::with_capacity(limit);
    while out.len() < limit {
        let take_remna = match (remna.peek(), core.peek()) {
            (Some(r), Some(c)) => r.at >= c.at,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let next = if take_remna {
            remna.next()
        } else {
            core.next()
        };
        match next {
            Some(item) => out.push(item),
            None => break,
        }
    }
    out
}

pub struct AdminOps<'a> {
    services: &'a Services,
}

impl<'a> AdminOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        AdminOps { services }
    }

    pub async fn dashboard(&self, sees: Sees) -> ServiceResult<DashboardView> {
        let include_money = sees.money;
        let currency = self.services.settings().currency;
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let offset = self.services.settings().remna_offset();
        let (subscriptions, today, revenue, stats_error) = match conn
            .subscriptions()
            .dashboard_in(now, currency, offset)
            .await
        {
            Ok(stats) => (
                Some(DashboardCounts {
                    active_paid: stats.active_paid,
                    trials: stats.trials,
                    expiring_soon: stats.expiring_soon,
                    never_connected: stats.never_connected,
                    limited: stats.limited,
                }),
                Some(DashboardToday {
                    payments: include_money.then_some(stats.payments_today),
                    new_clients: stats.new_clients_today,
                    new_clients_paid: include_money.then_some(stats.new_clients_paid_today),
                }),
                include_money.then(|| DashboardRevenue {
                    today: Money::new(stats.revenue_today_minor, currency),
                    yesterday: Money::new(stats.revenue_yesterday_minor, currency),
                    week: Money::new(stats.revenue_7d_minor, currency),
                    month: Money::new(stats.revenue_30d_minor, currency),
                    today_stars: stats.revenue_today_stars.0,
                    by_day: stats
                        .revenue_by_day
                        .iter()
                        .map(|day| DayAmount {
                            day: day.day,
                            amount: Money::new(day.minor, currency),
                        })
                        .collect(),
                }),
                None,
            ),
            Err(StoreError::NotImplemented(what)) => (None, None, None, Some(what.to_string())),
            Err(err) => return Err(err.into()),
        };
        let support_waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE status = 'open' AND last_author = 'client'",
        )
        .fetch_one(conn.conn())
        .await?;
        let orders_review: i64 =
            sqlx::query_scalar("SELECT count(*) FROM orders WHERE status = 'review'")
                .fetch_one(conn.conn())
                .await?;
        let stars_review: i64 =
            sqlx::query_scalar("SELECT count(*) FROM bot_star_events WHERE status = 'review'")
                .fetch_one(conn.conn())
                .await?;
        let refund_actions_failed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM provider_refunds WHERE action_status = 'failed'",
        )
        .fetch_one(conn.conn())
        .await?;
        let charges_review = conn.payments().charges_in_review().await?;
        let tariffs_on_sale: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tariffs WHERE deleted_at IS NULL AND on_sale)",
        )
        .fetch_one(conn.conn())
        .await?;
        let database_bytes: Option<i64> =
            sqlx::query_scalar("SELECT pg_database_size(current_database())")
                .fetch_one(conn.conn())
                .await
                .ok();
        drop(conn);
        let webhooks = if include_money {
            Some(self.services.inbox().webhook_attention().await?)
        } else {
            None
        };
        let server = if sees.infrastructure {
            let mut server = crate::server_load::probe().sample().await;
            server.database_bytes = database_bytes.and_then(|b| u64::try_from(b).ok());
            Some(server)
        } else {
            None
        };
        let (remna, remna_error) = match self.remna_overview(sees.infrastructure).await {
            Ok(overview) => (Some(overview), None),
            Err(err) => (None, sees.infrastructure.then(|| err.to_string())),
        };
        let money_count = |count: i64| include_money.then_some(count);
        Ok(DashboardView {
            currency,
            subscriptions,
            today,
            revenue,
            stats_error,
            support_waiting,
            orders_review: money_count(orders_review),
            stars_review: money_count(stars_review),
            refund_actions_failed: sees.refunds.then_some(refund_actions_failed),
            charges_review: money_count(charges_review),
            webhooks,
            remna,
            remna_error,
            server,
            setup: SetupState {
                remnawave: self.services.remna().is_ok(),
                bot: self.services.bot_token().is_some(),
                tariffs: tariffs_on_sale,
            },
        })
    }

    pub async fn support_waiting(&self) -> ServiceResult<i64> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM tickets WHERE status = 'open' AND last_author = 'client'",
        )
        .fetch_one(conn.conn())
        .await?)
    }

    async fn remna_overview(&self, with_nodes: bool) -> ServiceResult<RemnaOverview> {
        let remna = self.services.remna()?;
        let stats = remna.stats().await?;
        if !with_nodes {
            return Ok(RemnaOverview {
                online_now: stats.online_stats.online_now,
                users_total: stats.users.total_users,
                status_counts: stats.users.status_counts.into_iter().collect(),
                nodes_total: 0,
                nodes_online: 0,
                nodes: Vec::new(),
            });
        }
        let nodes = remna.nodes().await?;
        let down = self
            .services
            .node_watch()
            .down_since()
            .await
            .unwrap_or_default();
        let briefs: Vec<NodeBrief> = nodes
            .iter()
            .map(|node| NodeBrief {
                uuid: node.uuid,
                name: node.name.clone(),
                country_code: node.country_code.clone(),
                state: node.state(),
                users_online: node.users_online,
                traffic_alert: node.traffic_alert(),
                traffic_used_bytes: node.traffic_used_bytes,
                traffic_limit_bytes: node.traffic_limit_bytes.filter(|limit| *limit > 0),
                traffic_reset_day: node.traffic_reset_day,
                last_status_change: node.last_status_change,
                last_status_message: node.last_status_message.clone(),
                down_since: down
                    .get(&node.uuid)
                    .copied()
                    .filter(|_| matches!(node.state(), NodeState::Down | NodeState::Connecting)),
            })
            .collect();
        Ok(RemnaOverview {
            online_now: stats.online_stats.online_now,
            users_total: stats.users.total_users,
            status_counts: stats.users.status_counts.into_iter().collect(),
            nodes_total: briefs.len(),
            nodes_online: briefs
                .iter()
                .filter(|n| n.state == NodeState::Online)
                .count(),
            nodes: briefs,
        })
    }

    pub async fn feed(&self, query: &FeedQuery, sees: Sees) -> ServiceResult<FeedPage> {
        let limit = query.limit.unwrap_or(50).clamp(1, FEED_MAX);
        let fetch = i64::from(limit) + 1;
        let mut conn = self.services.store().conn().await?;
        let scopes = remna_scopes(query.kind, sees.infrastructure);
        let remna_rows: Vec<RemnaFeedRow> = if scopes.as_ref().is_some_and(Vec::is_empty) {
            Vec::new()
        } else {
            sqlx::query_as(
                "SELECT id, scope, event, occurred_at, subject_id, payload FROM remna_events \
                 WHERE ($1::bigint IS NULL OR id < $1) AND ($2::text[] IS NULL OR scope = ANY($2)) \
                 ORDER BY id DESC LIMIT $3",
            )
            .bind(query.remna_before)
            .bind(scopes)
            .bind(fetch)
            .fetch_all(conn.conn())
            .await?
        };
        let kinds = core_kinds(query.kind, sees.money);
        let core_rows: Vec<CoreFeedRow> = if kinds.as_ref().is_some_and(Vec::is_empty) {
            Vec::new()
        } else {
            sqlx::query_as(
                "SELECT id, occurred_at, client_id, payload FROM domain_events \
                 WHERE ($1::bigint IS NULL OR id < $1) AND ($2::text[] IS NULL OR kind = ANY($2)) \
                 ORDER BY id DESC LIMIT $3",
            )
            .bind(query.core_before)
            .bind(kinds)
            .bind(fetch)
            .fetch_all(conn.conn())
            .await?
        };
        let fetched = remna_rows.len() + core_rows.len();
        let mut remna_items = Vec::with_capacity(remna_rows.len());
        for row in remna_rows {
            let (title, data) = remna_feed_entry(&row.scope, &row.event, &row.payload);
            remna_items.push(FeedItem {
                source: FeedSource::Remnawave,
                id: row.id,
                at: row.occurred_at,
                kind: row.event,
                title,
                client_id: None,
                subject: row.subject_id,
                data,
                client: None,
                order: None,
                payment: None,
                ticket: None,
            });
        }
        let mut core_items = Vec::with_capacity(core_rows.len());
        for row in core_rows {
            let event: DomainEvent = match serde_json::from_value(row.payload.0) {
                Ok(event) => admin_event(event),
                Err(err) => {
                    tracing::warn!(event = row.id, error = %err, "событие ядра не разобрано, в ленте пропущено");
                    continue;
                }
            };
            core_items.push(FeedItem {
                source: FeedSource::Core,
                id: row.id,
                at: row.occurred_at,
                kind: event.kind().to_string(),
                title: core_feed_title(&event),
                client_id: row.client_id.map(ClientId),
                subject: None,
                data: serde_json::to_value(&event)
                    .map_err(|e| ServiceError::Internal(e.to_string()))?,
                client: None,
                order: None,
                payment: None,
                ticket: None,
            });
        }
        let mut items = merge_feed(remna_items, core_items, limit as usize);
        self.enrich_feed(&mut conn, &mut items).await?;
        let last = |source: FeedSource| {
            items
                .iter()
                .filter(|i| i.source == source)
                .map(|i| i.id)
                .min()
        };
        Ok(FeedPage {
            remna_before: last(FeedSource::Remnawave).or(query.remna_before),
            core_before: last(FeedSource::Core).or(query.core_before),
            has_more: fetched > items.len(),
            items,
        })
    }

    async fn enrich_feed(
        &self,
        conn: &mut sn_store::Conn,
        items: &mut [FeedItem],
    ) -> ServiceResult<()> {
        let client_ids: Vec<ClientId> = items.iter().filter_map(|item| item.client_id).collect();
        let people = conn.clients().people(&client_ids).await?;
        let payment_ids: Vec<PaymentId> = items
            .iter()
            .filter_map(
                |item| match serde_json::from_value::<DomainEvent>(item.data.clone()) {
                    Ok(DomainEvent::PaymentPaid { payment_id, .. }) => Some(payment_id),
                    _ => None,
                },
            )
            .collect();
        let payments: HashMap<PaymentId, Payment> = conn
            .payments()
            .by_ids(&payment_ids)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
        let order_ids: Vec<OrderId> = payments.values().map(|p| p.order_id).collect();
        let orders: HashMap<OrderId, sn_domain::order::Order> = conn
            .orders()
            .by_ids(&order_ids)
            .await?
            .into_iter()
            .map(|o| (o.id, o))
            .collect();
        let tariffs: HashMap<TariffId, String> = if orders.is_empty() {
            HashMap::new()
        } else {
            conn.tariffs()
                .list(true)
                .await?
                .into_iter()
                .map(|t| (t.id, t.name))
                .collect()
        };
        for item in items.iter_mut() {
            item.client = item.client_id.and_then(|id| people.get(&id).cloned());
            match serde_json::from_value::<DomainEvent>(item.data.clone()) {
                Ok(DomainEvent::PaymentPaid { payment_id, .. }) => {
                    if let Some(payment) = payments.get(&payment_id) {
                        item.order = orders
                            .get(&payment.order_id)
                            .map(|order| order_brief(order, &tariffs));
                        item.payment = Some(PaymentBrief {
                            id: payment.public_id,
                            number: payment.id,
                            status: payment.status,
                            amount: payment.amount,
                            provider: payment.provider.clone(),
                            method: payment.method.clone(),
                        });
                    }
                }
                Ok(DomainEvent::TicketOpened { ticket_id, .. })
                | Ok(DomainEvent::TicketAnswered { ticket_id, .. }) => {
                    if let Ok(ticket) = conn.tickets().get(ticket_id).await {
                        item.ticket = Some(TicketBrief {
                            id: ticket.id,
                            state: ticket.filter(),
                            subject: ticket.subject,
                            status: ticket.status,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub async fn payments_view(
        &self,
        filter: &PaymentFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<PaymentListItem>> {
        let list = self.payments(filter, page).await?;
        let mut conn = self.services.store().conn().await?;
        let client_ids: Vec<ClientId> = list.items.iter().map(|p| p.client_id).collect();
        let people = conn.clients().people(&client_ids).await?;
        let order_ids: Vec<OrderId> = list.items.iter().map(|p| p.order_id).collect();
        let orders: HashMap<OrderId, sn_domain::order::Order> = conn
            .orders()
            .by_ids(&order_ids)
            .await?
            .into_iter()
            .map(|o| (o.id, o))
            .collect();
        let tariffs: HashMap<TariffId, String> = conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        let failed: Vec<PaymentId> = list
            .items
            .iter()
            .filter(|p| p.status == PaymentStatus::Failed)
            .map(|p| p.id)
            .collect();
        let reasons = conn.payments().failure_reasons(&failed).await?;
        Ok(Page {
            next: list.next,
            items: list
                .items
                .into_iter()
                .map(|payment| PaymentListItem {
                    client: people.get(&payment.client_id).cloned(),
                    order: orders
                        .get(&payment.order_id)
                        .map(|order| order_brief(order, &tariffs)),
                    failure_reason: reasons.get(&payment.id).cloned(),
                    payment,
                })
                .collect(),
        })
    }

    pub async fn payments(
        &self,
        filter: &PaymentFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<Payment>> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<PaymentRow> = sqlx::query_as(
            "SELECT id, public_id, order_id, client_id, provider, method, amount_minor, currency, status, \
             provider_txid, pay_url, expires_at, paid_at, created_at, pay_method FROM payments \
             WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR provider = $2) \
             AND ($3::bigint IS NULL OR client_id = $3) AND ($4::bigint IS NULL OR id < $4) \
             AND ($6::bigint IS NULL OR id = $6) \
             ORDER BY id DESC LIMIT $5",
        )
        .bind(filter.status.map(PaymentStatus::as_str))
        .bind(filter.provider.as_deref().map(str::trim).filter(|p| !p.is_empty()))
        .bind(filter.client_id.map(|c| c.0))
        .bind(page.after)
        .bind(page.fetch_limit())
        .bind(filter.id.map(|p| p.0))
        .fetch_all(conn.conn())
        .await?;
        let items = rows
            .into_iter()
            .map(Payment::try_from)
            .collect::<ServiceResult<Vec<Payment>>>()?;
        Ok(Page::from_overfetch(items, page, |p| p.id.0))
    }

    pub async fn mark_paid(
        &self,
        id: PaymentId,
        request: &MarkPaidRequest,
        admin: AdminId,
    ) -> ServiceResult<MarkPaidResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(id).await?
        };
        match payment.provider.as_str() {
            providers::STARS => {
                return Err(ServiceError::rejected(
                    "payment.manual_stars",
                    "Оплату звёздами нельзя отметить вручную: её подтверждает только Telegram",
                ));
            }
            providers::BALANCE => {
                return Err(ServiceError::rejected(
                    "payment.manual_balance",
                    "Оплата с баланса проводится сразу, отмечать её не нужно",
                ));
            }
            _ => {}
        }
        if payment.amount.as_money().is_none() {
            return Err(ServiceError::rejected(
                "payment.manual_currency",
                "Вручную отмечается только оплата в валюте",
            ));
        }
        if !matches!(
            payment.status,
            PaymentStatus::Pending | PaymentStatus::Failed | PaymentStatus::Canceled
        ) {
            return Err(ServiceError::rejected(
                "payment.manual_status",
                format!("Платёж уже в статусе {}", payment.status.as_str()),
            ));
        }
        let txid = request
            .provider_txid
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(200).collect::<String>());
        let comment = request
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| c.chars().take(500).collect::<String>());
        let outcome = WebhookOutcome {
            event_key: format!("manual:{}:{}", admin.0, payment.id.0),
            payment: Some(PaymentRef::Payment(payment.id)),
            provider_txid: txid.clone(),
            status: ProviderStatus::Paid,
            amount: Some(payment.amount),
            saved_method: None,
            reply: WebhookReply::ok(),
            signal: None,
        };
        let accepted = self
            .services
            .inbox()
            .apply_outcome(payment.provider.as_str(), &outcome)
            .await?;
        let result = match accepted {
            AcceptResult::Accepted { order_id, .. } => MarkPaidResult::Accepted { order_id },
            AcceptResult::AlreadyApplied { .. } => MarkPaidResult::AlreadyApplied,
            AcceptResult::StatusChanged { status, .. } => {
                return Err(ServiceError::rejected(
                    "payment.manual_status",
                    format!("Платёж перешёл в статус {}", status.as_str()),
                ));
            }
            AcceptResult::Ignored { reason } => {
                return Err(ServiceError::rejected("payment.manual_ignored", reason));
            }
        };
        let mut conn = self.services.store().conn().await?;
        conn.audit()
            .record(&Actor::Admin(admin).audit(
                "payment.mark_paid",
                "payment",
                Some(payment.id.0),
                serde_json::json!({
                    "client_id": payment.client_id,
                    "previous_status": payment.status,
                    "provider_txid": txid,
                    "comment": comment,
                }),
            ))
            .await?;
        Ok(result)
    }

    pub async fn cancel_payment(
        &self,
        id: PaymentId,
        request: &CancelPaymentRequest,
        admin: AdminId,
    ) -> ServiceResult<CancelPaymentResult> {
        let not_waiting = || {
            ServiceError::rejected(
                "payment.cancel_status",
                "Этот счёт уже не ждёт оплаты, обновите список",
            )
        };
        let comment = request
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| c.chars().take(500).collect::<String>());
        let ctx = self.services.module_context();
        let mut tx = self.services.store().begin().await?;
        let payment = tx.payments().lock(id).await?;
        if payment.status != PaymentStatus::Pending {
            return Err(not_waiting());
        }
        if payment.method.as_deref() == Some(AUTORENEW_METHOD) {
            return Err(ServiceError::rejected(
                "payment.cancel_autorenew",
                "Списание автопродления закроется само. Если деньги списаны, отметьте его оплаченным",
            ));
        }
        if tx.orders().get(payment.order_id).await?.status == OrderStatus::Review {
            return Err(ServiceError::rejected(
                "payment.cancel_review",
                "Заказ задержан проверкой, решите его в «Деньги → Ждут решения»",
            ));
        }
        match tx
            .payments()
            .transition(
                payment.id,
                PaymentStatus::Canceled,
                None,
                Some(ADMIN_CANCELED_REASON),
            )
            .await
        {
            Ok(change) if change.applied() => {}
            Ok(_) | Err(StoreError::Invalid(_)) => return Err(not_waiting()),
            Err(err) => return Err(err.into()),
        }
        let order_canceled = crate::checkout::cancel_if_unpaid(
            self.services.modules(),
            &ctx,
            &mut tx,
            payment.order_id,
        )
        .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "payment.cancel",
                "payment",
                Some(payment.id.0),
                serde_json::json!({
                    "client_id": payment.client_id,
                    "order_id": payment.order_id,
                    "provider": payment.provider,
                    "amount": payment.amount,
                    "order_canceled": order_canceled,
                    "comment": comment,
                }),
            ))
            .await?;
        tx.commit().await?;
        crate::checkout::cancel_at_providers(self.services, &[payment.id]).await;
        Ok(CancelPaymentResult { order_canceled })
    }

    pub async fn refund(
        &self,
        id: PaymentId,
        request: &RefundRequest,
        admin: AdminId,
    ) -> ServiceResult<RefundResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(id).await?
        };
        let replay = match request.key {
            Some(key) => self.services.inbox().refund_keyed(payment.id, key).await?,
            None => false,
        };
        if !replay
            && !matches!(
                payment.status,
                PaymentStatus::Paid | PaymentStatus::PartiallyRefunded
            )
        {
            return Err(ServiceError::rejected(
                "payment.refund_status",
                "Вернуть можно только оплаченный платёж",
            ));
        }
        if request
            .amount_minor
            .is_some_and(|amount| amount <= 0 || amount > payment.amount.minor)
        {
            return Err(ServiceError::rejected(
                "payment.refund_amount",
                "Сумма возврата больше суммы платежа",
            ));
        }
        let command = RefundCommand {
            amount: request
                .amount_minor
                .map(|minor| Amount::new(minor, payment.amount.currency)),
            method: request.method,
            subscription_action: request.subscription_action,
            reason: request.reason.clone(),
            key: request.key,
            outside: request.outside,
        };
        self.services.inbox().refund(id, &command, admin).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn лента_не_показывает_ссылку_подписки_из_события() {
        let payload = serde_json::json!({
            "scope": "user", "event": "user.limited", "timestamp": "2026-11-16T14:52:00Z",
            "data": { "user": { "id": 42, "username": "sn_abc", "status": "LIMITED",
                "subscriptionUrl": "https://sub.example.com/secret", "expireAt": "2027-02-14T00:00:00Z" },
                "meta": {} }
        });
        let (title, data) = remna_feed_entry("user", "user.limited", &payload);
        assert_eq!(title, "У sn_abc закончился трафик");
        assert_eq!(data["username"], "sn_abc");
        assert!(!data.to_string().contains("secret"));
    }

    #[test]
    fn заголовки_событий_нод_и_оплаты_серверов() {
        let node = serde_json::json!({ "data": { "name": "NL-2", "address": "198.51.100.22" } });
        assert_eq!(
            remna_feed_entry("node", "node.connection_lost", &node).0,
            "Нода NL-2 не отвечает"
        );
        let crm = serde_json::json!({ "data": { "nodeName": "FI-1", "providerName": "Hetzner",
            "nextBillingAt": "2026-11-18T00:00:00Z" } });
        assert_eq!(
            remna_feed_entry("crm", "crm.infra_billing_node_payment_in_7_days", &crm).0,
            "Скоро оплата сервера FI-1 (Hetzner)"
        );
    }

    #[test]
    fn без_права_на_деньги_денежные_события_не_выбираются() {
        let kinds = core_kinds(FeedKind::All, false).unwrap();
        assert!(!kinds.iter().any(|k| MONEY_EVENTS.contains(&k.as_str())));
        assert!(core_kinds(FeedKind::All, true).is_none());
        assert!(core_kinds(FeedKind::Nodes, true).unwrap().is_empty());
        assert_eq!(
            core_kinds(FeedKind::Payments, false).unwrap(),
            vec!["order_fulfilled".to_string()]
        );
    }
}
