use crate::Actor;
use crate::autorenew::AUTORENEW_METHOD;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::site_signup;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::{
    Client, ClientStatus, Identity, IdentityKind, Locale, NewIdentity, RegistrationSource,
    TelegramProfile, normalize_identity,
};
use sn_domain::events::DomainEvent;
use sn_domain::ids::{
    AdminId, CategoryId, ClientId, OrderId, PaymentId, PeriodId, SubscriptionId, TariffId, TicketId,
};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Money};
use sn_domain::order::{NewOrder, Order, OrderChannel, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{PaymentStatus, ProviderId};
use sn_domain::plan::{CurrentTariff, PlanKind, classify};
use sn_domain::pricing::{Quote, QuoteLine, QuoteLineKind};
use sn_domain::subscription::Subscription;
use sn_domain::support::{TicketFilter, TicketStatus};
use sn_domain::tariff::{DevicePolicy, months_for_days};
use sn_domain::traffic::TrafficPolicy;
use sn_modules::catalog::antibot::signups::{self, Via};
use sn_store::{
    ClientFilter, ClientHistoryEntry, ClientListItem, ClientPerson, Db, LockSpace, NewClient,
    StoreError, Tx,
};
use std::collections::HashMap;
use uuid::Uuid;

mod payments;
#[cfg(test)]
mod tests;

pub(crate) use payments::method_title;
pub use payments::{ClientPayment, ClientPayments, Purchase};

pub const MAX_BLOCK_REASON: usize = 500;
pub const RECEIPT_EMAIL_INVALID: &str = "receipt.email_invalid";
pub const MAX_MESSAGE: usize = 3_500;
const MESSAGE_TEMPLATE: &str = "text";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderBrief {
    pub id: Uuid,
    pub kind: OrderKind,
    pub status: OrderStatus,
    pub label: Option<String>,
    pub tariff_id: Option<TariffId>,
    pub tariff_name: Option<String>,
    pub days: Option<u32>,
    pub months: Option<u32>,
    pub extra_devices: u32,
    pub total: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentBrief {
    pub id: Uuid,
    pub number: PaymentId,
    pub status: PaymentStatus,
    pub amount: Amount,
    pub provider: ProviderId,
    pub method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TicketBrief {
    pub id: TicketId,
    pub subject: Option<String>,
    pub status: TicketStatus,
    pub state: TicketFilter,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryRecord {
    Payment {
        payment: PaymentBrief,
        order: Option<OrderBrief>,
        #[serde(skip_serializing_if = "Option::is_none")]
        review: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure_reason: Option<String>,
    },
    Order {
        order: OrderBrief,
    },
    Event {
        kind: String,
        event: DomainEvent,
        ticket: Option<TicketBrief>,
    },
    Audit {
        action: String,
        actor_kind: String,
        actor_id: Option<i64>,
        actor_email: Option<String>,
        entity_kind: Option<String>,
        entity_id: Option<i64>,
        details: serde_json::Value,
    },
}

pub(crate) fn admin_event(event: DomainEvent) -> DomainEvent {
    match event {
        DomainEvent::ClientRegistered {
            client_id,
            source,
            start_param,
        } => DomainEvent::ClientRegistered {
            client_id,
            source,
            start_param: start_param
                .map(|raw| sn_modules::catalog::start_param::masked(&raw))
                .filter(|masked| !masked.is_empty()),
        },
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryItem {
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub record: HistoryRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub next: Option<i64>,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffBrief {
    pub id: TariffId,
    pub category_id: CategoryId,
    pub name: String,
    pub on_sale: bool,
    pub devices: DevicePolicy,
    pub traffic: TrafficPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardStats {
    pub paid_total: Money,
    pub payments_count: i64,
    pub referrals_count: i64,
    pub referrer: Option<ClientPerson>,
    pub cabinet_seen_at: Option<DateTime<Utc>>,
    pub miniapp_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantPeriod {
    pub id: PeriodId,
    pub days: u32,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantTariff {
    pub id: TariffId,
    pub name: String,
    pub category_id: CategoryId,
    pub category_limit: Option<u32>,
    pub on_sale: bool,
    pub devices: DevicePolicy,
    pub traffic: TrafficPolicy,
    pub periods: Vec<GrantPeriod>,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GrantRequest {
    pub tariff_id: TariffId,
    #[serde(default)]
    pub period_id: Option<PeriodId>,
    #[serde(default)]
    pub days: Option<u32>,
    #[serde(default)]
    pub subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantResult {
    pub order_id: Uuid,
    pub plan: PlanKind,
    pub days: u32,
}

pub const MAX_GRANT_DAYS: u32 = 3650;

pub(crate) fn order_brief(order: &Order, tariffs: &HashMap<TariffId, String>) -> OrderBrief {
    let tariff_id = match &order.intent {
        PurchaseIntent::Subscription { tariff_id, .. }
        | PurchaseIntent::AdminGrant { tariff_id, .. } => Some(*tariff_id),
        PurchaseIntent::ModuleGrant { tariff_id, .. } => *tariff_id,
        _ => order.snapshot.as_ref().map(|snapshot| snapshot.tariff_id),
    };
    let days = (order.quote.days > 0).then_some(order.quote.days);
    OrderBrief {
        id: order.public_id,
        kind: order.kind,
        status: order.status,
        label: order
            .quote
            .lines
            .first()
            .map(|line| line.shown_label())
            .or_else(|| match &order.intent {
                PurchaseIntent::Imported { label, .. } => label.clone(),
                _ => None,
            }),
        tariff_id,
        tariff_name: tariff_id
            .and_then(|id| tariffs.get(&id).cloned())
            .or_else(|| {
                order
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.name.clone())
            }),
        days,
        months: days.filter(|d| *d >= 28).map(months_for_days),
        extra_devices: order.quote.extra_devices,
        total: order.quote.total,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaymentStatusView {
    pub payment_id: Uuid,
    pub status: PaymentStatus,
    pub amount: Amount,
    pub provider: ProviderId,
    pub order_id: Uuid,
    pub order_kind: OrderKind,
    pub order_status: OrderStatus,
    pub subscription_id: Option<SubscriptionId>,
    pub pay_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub paid_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientCard {
    pub client: Client,
    pub identities: Vec<Identity>,
    pub balance: Money,
    pub balance_held: Money,
    pub referral_balance: Money,
    pub subscriptions: Vec<Subscription>,
    pub tariffs: Vec<TariffBrief>,
    pub stats: CardStats,
    pub bot_available: bool,
    pub two_factor: crate::two_factor::TwoFactor,
    pub multi_subscriptions: bool,
    pub receipt_email: Option<String>,
}

const BLOCKED_REASON: &str = "клиент заблокирован";

struct CanceledInvoices {
    payments: Vec<PaymentId>,
    orders: Vec<OrderId>,
    kept: Vec<PaymentId>,
}

async fn cancel_open_invoices(tx: &mut Tx, client_id: ClientId) -> ServiceResult<CanceledInvoices> {
    let pending: Vec<(i64, bool)> = sqlx::query_as(
        "SELECT id, method IS NOT DISTINCT FROM $2 FROM payments \
         WHERE client_id = $1 AND status = 'pending' ORDER BY id",
    )
    .bind(client_id.0)
    .bind(AUTORENEW_METHOD)
    .fetch_all(tx.conn())
    .await?;
    let mut payments = Vec::new();
    let mut kept = Vec::new();
    for (id, in_flight) in pending {
        let id = PaymentId(id);
        if in_flight {
            kept.push(id);
            continue;
        }
        match tx
            .payments()
            .transition(id, PaymentStatus::Canceled, None, Some(BLOCKED_REASON))
            .await
        {
            Ok(change) if change.applied() => payments.push(id),
            Ok(_) | Err(StoreError::Invalid(_)) => {}
            Err(err) => return Err(err.into()),
        }
    }
    let open: Vec<i64> = sqlx::query_scalar(
        "SELECT o.id FROM orders o WHERE o.client_id = $1 AND o.status = 'pending' \
         AND NOT EXISTS (SELECT 1 FROM payments p WHERE p.order_id = o.id \
         AND p.status = 'pending' AND p.method = $2) ORDER BY o.id",
    )
    .bind(client_id.0)
    .bind(AUTORENEW_METHOD)
    .fetch_all(tx.conn())
    .await?;
    let mut orders = Vec::new();
    for id in open.into_iter().map(OrderId) {
        match tx.orders().transition(id, OrderStatus::Canceled).await {
            Ok((_, true)) => orders.push(id),
            Ok(_) | Err(StoreError::Invalid(_)) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(CanceledInvoices {
        payments,
        orders,
        kept,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteVisit {
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entered {
    Ready(Client, bool),
    Busy,
    Refused(signups::Refusal),
}

pub struct ClientOps<'a> {
    pub(crate) services: &'a Services,
}

impl<'a> ClientOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ClientOps { services }
    }

    pub async fn ensure_telegram(
        &self,
        profile: &TelegramProfile,
        source: RegistrationSource,
        start_param: Option<&str>,
    ) -> ServiceResult<(Client, bool)> {
        self.ensure_telegram_admitted(profile, source, start_param, || true)
            .await?
            .ok_or_else(|| ServiceError::Internal("регистрация не допущена".into()))
    }

    pub async fn ensure_telegram_admitted(
        &self,
        profile: &TelegramProfile,
        source: RegistrationSource,
        start_param: Option<&str>,
        admit: impl FnOnce() -> bool + Send,
    ) -> ServiceResult<Option<(Client, bool)>> {
        Ok(
            match self
                .ensure_telegram_from(profile, source, start_param, None, admit)
                .await?
            {
                Entered::Ready(client, created) => Some((client, created)),
                Entered::Busy | Entered::Refused(_) => None,
            },
        )
    }

    pub async fn ensure_telegram_from(
        &self,
        profile: &TelegramProfile,
        source: RegistrationSource,
        start_param: Option<&str>,
        site: Option<&SiteVisit>,
        admit: impl FnOnce() -> bool + Send,
    ) -> ServiceResult<Entered> {
        let identity = NewIdentity::telegram(profile);
        {
            let mut conn = self.services.store().conn().await?;
            if let Some(client) = conn
                .clients()
                .find_by_identity(IdentityKind::Telegram, &identity.value)
                .await?
            {
                conn.clients()
                    .update_identity_meta(IdentityKind::Telegram, &identity.value, &identity.meta)
                    .await?;
                return Ok(Entered::Ready(client, false));
            }
        }
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Telegram, profile.id).await?;
        if let Some(client) = tx
            .clients()
            .find_by_identity(IdentityKind::Telegram, &identity.value)
            .await?
        {
            tx.clients()
                .update_identity_meta(IdentityKind::Telegram, &identity.value, &identity.meta)
                .await?;
            tx.commit().await?;
            return Ok(Entered::Ready(client, false));
        }
        let admission = match site {
            None => None,
            Some(visit) => match signups::admit(&mut tx, visit.ip.as_deref(), false).await? {
                Ok(admission) => admission,
                Err(refusal) => {
                    tx.rollback().await?;
                    site_signup::count(self.services, refusal.as_str()).await;
                    return Ok(Entered::Refused(refusal));
                }
            },
        };
        if !admit() {
            tx.rollback().await?;
            return Ok(Entered::Busy);
        }
        let client = tx
            .clients()
            .create(&NewClient {
                locale: self
                    .services
                    .settings()
                    .languages
                    .detect(profile.language_code.as_deref()),
                registration_source: source,
                note: None,
            })
            .await?;
        tx.clients().add_identity(client.id, &identity).await?;
        signups::record(&mut tx, admission, client.id, Via::Telegram).await?;
        tx.events()
            .push(&DomainEvent::ClientRegistered {
                client_id: client.id,
                source,
                start_param: start_param.map(str::to_string),
            })
            .await?;
        tx.commit().await?;
        Ok(Entered::Ready(client, true))
    }

    pub async fn card(&self, id: ClientId) -> ServiceResult<ClientCard> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(id).await?;
        let identities = conn.clients().identities(id).await?;
        let balance = conn.ledger().balance(id, Wallet::Main, currency).await?;
        let balance_held = Money::new(
            conn.payments()
                .held_refunds_minor(id, currency.into())
                .await?,
            currency,
        );
        let referral_balance = conn
            .ledger()
            .balance(id, Wallet::Referral, currency)
            .await?;
        let subscriptions = conn.subscriptions().by_client(id).await?;
        let wanted: Vec<TariffId> = subscriptions.iter().filter_map(|s| s.tariff_id).collect();
        let tariffs = conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .filter(|t| wanted.contains(&t.id))
            .map(|t| TariffBrief {
                id: t.id,
                category_id: t.category_id,
                name: t.name,
                on_sale: t.on_sale,
                devices: t.devices,
                traffic: t.traffic,
            })
            .collect();
        let stats = conn.clients().card_stats(id, currency).await?;
        let two_factor = crate::two_factor::TwoFactor::of(conn.clients().totp(id).await?.as_ref());
        let receipt_email = conn.clients().receipt_email(id).await?;
        Ok(ClientCard {
            receipt_email,
            client,
            identities,
            balance,
            balance_held,
            referral_balance,
            subscriptions,
            tariffs,
            stats: CardStats {
                paid_total: Money::new(stats.paid_total_minor, currency),
                payments_count: stats.payments_count,
                referrals_count: stats.referrals_count,
                referrer: stats.referrer,
                cabinet_seen_at: stats.cabinet_seen_at,
                miniapp_seen_at: stats.miniapp_seen_at,
            },
            bot_available: self.services.bot_token().is_some(),
            two_factor,
            multi_subscriptions: self.services.settings().multi_subscriptions,
        })
    }

    pub async fn set_receipt_email(
        &self,
        id: ClientId,
        raw: &str,
        actor: Actor,
    ) -> ServiceResult<String> {
        let email = normalize_identity(IdentityKind::Email, raw).map_err(|_| {
            ServiceError::rejected(
                RECEIPT_EMAIL_INVALID,
                "Похоже, в почте опечатка: нужен адрес вида name@example.com",
            )
        })?;
        let mut tx = self.services.store().begin().await?;
        tx.clients().get(id).await?;
        let before = tx.clients().receipt_email(id).await?;
        if before.as_deref() != Some(email.as_str()) {
            tx.clients().set_receipt_email(id, Some(&email)).await?;
            tx.audit()
                .record(&actor.audit(
                    "client.receipt_email",
                    "client",
                    Some(id.0),
                    serde_json::json!({
                        "email": sn_domain::client::mask_email(&email),
                        "changed": before.is_some(),
                    }),
                ))
                .await?;
        }
        tx.commit().await?;
        Ok(email)
    }

    pub async fn set_note(
        &self,
        id: ClientId,
        note: Option<&str>,
        actor: Actor,
    ) -> ServiceResult<()> {
        let note = note.map(str::trim).filter(|n| !n.is_empty());
        if note.is_some_and(|n| n.chars().count() > 2_000) {
            return Err(ServiceError::rejected(
                "client.note_too_long",
                "заметка длиннее 2000 символов",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        tx.clients().get(id).await?;
        tx.clients().set_note(id, note).await?;
        tx.audit()
            .record(&actor.audit(
                "client.note",
                "client",
                Some(id.0),
                serde_json::json!({ "cleared": note.is_none() }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn grant_options(&self) -> ServiceResult<Vec<GrantTariff>> {
        let mut conn = self.services.store().conn().await?;
        let limits: std::collections::HashMap<CategoryId, Option<u32>> = conn
            .tariffs()
            .categories(true)
            .await?
            .into_iter()
            .map(|category| (category.id, category.subscription_limit))
            .collect();
        Ok(conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .map(|tariff| GrantTariff {
                category_limit: limits.get(&tariff.category_id).copied().flatten(),
                ready: !tariff.squads.is_empty(),
                periods: tariff
                    .periods
                    .iter()
                    .map(|period| GrantPeriod {
                        id: period.id,
                        days: period.days,
                        active: period.active,
                    })
                    .collect(),
                id: tariff.id,
                name: tariff.name,
                category_id: tariff.category_id,
                on_sale: tariff.on_sale,
                devices: tariff.devices,
                traffic: tariff.traffic,
            })
            .collect())
    }

    pub async fn grant(
        &self,
        client_id: ClientId,
        request: &GrantRequest,
        actor: Actor,
    ) -> ServiceResult<GrantResult> {
        self.services.remna()?;
        let note = request
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(500).collect::<String>());
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Client, client_id.0).await?;
        let client = tx.clients().get(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::rejected(
                "grant.client_blocked",
                "клиент заблокирован: сначала разблокируйте его",
            ));
        }
        let tariff = match tx.tariffs().get(request.tariff_id).await {
            Ok(tariff) => tariff,
            Err(StoreError::NotFound(_)) => return Err(ServiceError::NotFound("тариф")),
            Err(err) => return Err(err.into()),
        };
        if tariff.squads.is_empty() {
            return Err(ServiceError::rejected(
                "grant.no_squads",
                "у тарифа нет сквадов: подписка выдалась бы без серверов",
            ));
        }
        let base = match request.period_id {
            Some(id) => tariff.period(id).ok_or_else(|| {
                ServiceError::rejected("grant.period", "у тарифа нет такого срока")
            })?,
            None => tariff
                .periods
                .iter()
                .find(|p| p.active)
                .or_else(|| tariff.periods.first())
                .ok_or_else(|| {
                    ServiceError::rejected("grant.period", "у тарифа нет ни одного срока")
                })?,
        };
        let days = match (request.period_id, request.days) {
            (Some(_), _) => base.days,
            (None, Some(days)) if (1..=MAX_GRANT_DAYS).contains(&days) => days,
            _ => {
                return Err(ServiceError::rejected(
                    "grant.days",
                    format!("выберите срок тарифа или укажите от 1 до {MAX_GRANT_DAYS} дней"),
                ));
            }
        };
        let current = match request.subscription_id {
            Some(id) => {
                let subscription = tx.subscriptions().get(id).await?;
                if subscription.client_id != client_id {
                    return Err(ServiceError::NotFound("подписка"));
                }
                if subscription.ended_at.is_some() {
                    return Err(ServiceError::rejected(
                        "subscription.ended",
                        "подписка завершена",
                    ));
                }
                let category = match subscription.tariff_id {
                    Some(id) => match tx.tariffs().get(id).await {
                        Ok(t) => Some(t.category_id),
                        Err(StoreError::NotFound(_)) => None,
                        Err(err) => return Err(err.into()),
                    },
                    None => None,
                };
                Some((subscription, category))
            }
            None => None,
        };
        let plan = classify(
            tariff.category_id,
            current.as_ref().map(|(s, category)| CurrentTariff {
                kind: s.kind,
                category_id: *category,
            }),
        );
        let mut snapshot = tariff.snapshot(base);
        snapshot.days = days;
        let currency = self.services.settings().currency;
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: current.as_ref().map(|(s, _)| s.id),
                kind: OrderKind::AdminGrant,
                intent: PurchaseIntent::AdminGrant {
                    tariff_id: tariff.id,
                    period_id: request.period_id,
                    days,
                    note: note.clone(),
                },
                quote: Quote {
                    currency,
                    lines: vec![QuoteLine {
                        kind: QuoteLineKind::Period {
                            tariff_id: tariff.id,
                            period_id: base.id,
                            days,
                            months: months_for_days(days),
                        },
                        label: format!("{} · {}", tariff.name, sn_domain::format::days(days)),
                        amount: Money::zero(currency),
                        stars: None,
                    }],
                    subtotal: Money::zero(currency),
                    discount: Money::zero(currency),
                    total: Money::zero(currency),
                    stars: None,
                    days,
                    extra_devices: 0,
                },
                snapshot: Some(snapshot),
            })
            .await?;
        tx.orders()
            .set_channel(order.id, OrderChannel::Admin)
            .await?;
        tx.orders().transition(order.id, OrderStatus::Paid).await?;
        tx.jobs()
            .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
            .await?;
        tx.audit()
            .record(&actor.audit(
                "client.grant",
                "client",
                Some(client_id.0),
                serde_json::json!({
                    "order_id": order.public_id,
                    "tariff_id": tariff.id,
                    "tariff": tariff.name,
                    "days": days,
                    "plan": plan,
                    "subscription_id": current.as_ref().map(|(s, _)| s.id),
                    "note": note,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(GrantResult {
            order_id: order.public_id,
            plan,
            days,
        })
    }

    pub async fn delete(&self, id: ClientId, actor: Actor) -> ServiceResult<()> {
        let live: Vec<SubscriptionId> = {
            let mut conn = self.services.store().conn().await?;
            conn.clients().get(id).await?;
            conn.subscriptions()
                .by_client(id)
                .await?
                .into_iter()
                .filter(|s| s.ended_at.is_none())
                .map(|s| s.id)
                .collect()
        };
        for subscription in &live {
            self.services
                .subscriptions()
                .end_by_admin(*subscription, actor)
                .await?;
        }
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(id).await?;
        tx.sessions().revoke_all_client(id).await?;
        let invoices = cancel_open_invoices(&mut tx, id).await?;
        let ctx = self.services.module_context();
        for order_id in &invoices.orders {
            let order = tx.orders().get(*order_id).await?;
            self.services
                .modules()
                .order_canceled(&ctx, &mut tx, &order)
                .await?;
        }
        let identities = tx.clients().delete_identities(id).await?;
        tx.clients().soft_delete(id).await?;
        tx.audit()
            .record(&actor.audit(
                "client.deleted",
                "client",
                Some(id.0),
                serde_json::json!({
                    "ended_subscriptions": live,
                    "identities": identities,
                    "canceled_payments": invoices.payments,
                    "canceled_orders": invoices.orders,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn send_message(&self, id: ClientId, text: &str, actor: Actor) -> ServiceResult<()> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_MESSAGE {
            return Err(ServiceError::rejected(
                "client.message",
                format!("сообщение должно быть от 1 до {MAX_MESSAGE} символов"),
            ));
        }
        if self.services.bot_token().is_none() {
            return Err(ServiceError::rejected(
                "client.bot_missing",
                "бот не подключён: сообщение некуда отправить",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().get(id).await?;
        let telegram = tx
            .clients()
            .identities(id)
            .await?
            .into_iter()
            .any(|identity| identity.kind == IdentityKind::Telegram);
        if !telegram {
            return Err(ServiceError::rejected(
                "client.no_telegram",
                "у клиента нет Telegram: написать можно только через бота",
            ));
        }
        if client.bot_unreachable {
            return Err(ServiceError::rejected(
                "client.bot_unreachable",
                "клиент остановил бота, сообщение не дойдёт",
            ));
        }
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id: id,
                    template: MESSAGE_TEMPLATE.into(),
                    params: serde_json::json!({ "text": text }),
                },
                None,
            )
            .await?;
        tx.audit()
            .record(&actor.audit(
                "client.message",
                "client",
                Some(id.0),
                serde_json::json!({ "text": text.chars().take(500).collect::<String>() }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn adjust_balance(
        &self,
        id: ClientId,
        delta: Money,
        comment: &str,
        admin: AdminId,
        idempotency_key: &str,
    ) -> ServiceResult<Money> {
        let currency = self.services.settings().currency;
        if delta.currency != currency {
            return Err(ServiceError::rejected(
                "balance.currency",
                format!("баланс ведётся в {currency}"),
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: id,
                wallet: Wallet::Main,
                amount: delta,
                kind: LedgerKind::AdminAdjust,
                order_id: None,
                payment_id: None,
                admin_id: Some(admin),
                comment: Some(comment.to_string()),
                idempotency_key: Some(format!("admin_adjust:{idempotency_key}")),
            })
            .await?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: id,
                    wallet: Wallet::Main,
                    kind: LedgerKind::AdminAdjust,
                    delta,
                })
                .await?;
        }
        tx.commit().await?;
        Ok(appended.balance_after)
    }

    pub async fn search(
        &self,
        filter: &ClientFilter,
        page: PageRequest,
    ) -> ServiceResult<Page<ClientListItem>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .clients()
            .search_at(
                filter,
                page,
                self.services.now(),
                self.services.settings().currency,
            )
            .await?)
    }

    pub async fn segment_counts(&self) -> ServiceResult<sn_store::SegmentCounts> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.clients().segment_counts(self.services.now()).await?)
    }

    pub async fn filter_options(&self) -> ServiceResult<sn_store::FilterOptions> {
        let mut conn = self.services.store().conn().await?;
        Ok(sn_store::filter_options(conn.conn()).await?)
    }

    pub async fn segment_counts_where(
        &self,
        filter: &ClientFilter,
    ) -> ServiceResult<sn_store::SegmentCounts> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .clients()
            .segment_counts_where(
                filter,
                self.services.now(),
                self.services.settings().currency,
            )
            .await?)
    }

    pub async fn tariff_names(
        &self,
    ) -> ServiceResult<std::collections::HashMap<sn_domain::ids::TariffId, String>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect())
    }

    pub async fn set_locale(&self, id: ClientId, locale: Locale) -> ServiceResult<Locale> {
        if !self.services.settings().languages.allows(locale) {
            return Err(ServiceError::rejected(
                "client.locale",
                "Этот язык сейчас выключен",
            ));
        }
        let mut conn = self.services.store().conn().await?;
        conn.clients().set_locale(id, locale).await?;
        Ok(locale)
    }

    pub async fn block(
        &self,
        id: ClientId,
        reason: Option<&str>,
        actor: Actor,
    ) -> ServiceResult<()> {
        let reason = reason.map(str::trim).filter(|r| !r.is_empty());
        if reason.is_some_and(|r| r.chars().count() > MAX_BLOCK_REASON) {
            return Err(ServiceError::rejected(
                "client.block_reason",
                format!("причина длиннее {MAX_BLOCK_REASON} символов"),
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().lock(id).await?;
        let changed = !client.is_blocked();
        tx.clients()
            .set_status(id, ClientStatus::Blocked, reason)
            .await?;
        tx.sessions().revoke_all_client(id).await?;
        let invoices = cancel_open_invoices(&mut tx, id).await?;
        let ctx = self.services.module_context();
        for order_id in &invoices.orders {
            let order = tx.orders().get(*order_id).await?;
            self.services
                .modules()
                .order_canceled(&ctx, &mut tx, &order)
                .await?;
        }
        if changed {
            tx.events()
                .push(&DomainEvent::ClientBlocked {
                    client_id: id,
                    reason: reason.map(str::to_string),
                })
                .await?;
        }
        tx.audit()
            .record(&actor.audit(
                "client.block",
                "client",
                Some(id.0),
                serde_json::json!({
                    "reason": reason,
                    "changed": changed,
                    "canceled_payments": invoices.payments,
                    "canceled_orders": invoices.orders,
                    "kept_autorenew_payments": invoices.kept,
                }),
            ))
            .await?;
        tx.commit().await?;
        if let Err(err) = self
            .services
            .renewals()
            .cancel_client(id, crate::renewals::CancelSource::Block, actor)
            .await
        {
            tracing::warn!(client = %id, error = %err, "автопродление заблокированного клиента не отменено, повторит сверка");
        }
        self.services.subscriptions().block_access(id, reason).await
    }

    pub async fn unblock(&self, id: ClientId, actor: Actor) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().lock(id).await?;
        let changed = client.is_blocked();
        tx.clients()
            .set_status(id, ClientStatus::Active, None)
            .await?;
        if changed {
            tx.events()
                .push(&DomainEvent::ClientUnblocked { client_id: id })
                .await?;
        }
        tx.audit()
            .record(&actor.audit(
                "client.unblock",
                "client",
                Some(id.0),
                serde_json::json!({ "changed": changed }),
            ))
            .await?;
        tx.commit().await?;
        self.services.subscriptions().restore_access(id).await?;
        self.resume_charge_polls(id).await?;
        let resumed = self.services.fulfillment().resume_blocked(id).await?;
        if !resumed.queued.is_empty() || !resumed.kept.is_empty() {
            tracing::info!(client = %id, queued = ?resumed.queued, kept = ?resumed.kept, "заказы, ждавшие разблокировки, поставлены на выдачу");
        }
        Ok(())
    }

    async fn resume_charge_polls(&self, id: ClientId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let waiting: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM payments WHERE client_id = $1 AND status = 'pending' AND method = $2 \
             AND (expires_at IS NULL OR expires_at > $3) ORDER BY id",
        )
        .bind(id.0)
        .bind(AUTORENEW_METHOD)
        .bind(self.services.now())
        .fetch_all(conn.conn())
        .await?;
        for payment_id in waiting.into_iter().map(PaymentId) {
            let queued = conn
                .jobs()
                .ensure_queued(&JobPayload::PollPayment { payment_id })
                .await?;
            if queued.armed() {
                tracing::info!(client = %id, payment = %payment_id, job = %queued.id(), "опрос автосписания возобновлён после разблокировки");
            }
        }
        Ok(())
    }

    pub async fn history(
        &self,
        id: ClientId,
        page: PageRequest,
        include_money: bool,
    ) -> ServiceResult<HistoryPage> {
        let mut conn = self.services.store().conn().await?;
        conn.clients().get(id).await?;
        let feed = conn.clients().history(id, page, include_money).await?;
        let total = conn.clients().history_count(id, include_money).await?;
        let tariffs: HashMap<TariffId, String> = conn
            .tariffs()
            .list(true)
            .await?
            .into_iter()
            .map(|t| (t.id, t.name))
            .collect();
        let order_ids: Vec<OrderId> = feed
            .items
            .iter()
            .filter_map(|item| match &item.entry {
                ClientHistoryEntry::Payment(payment) => Some(payment.order_id),
                _ => None,
            })
            .collect();
        let orders: HashMap<OrderId, Order> = conn
            .orders()
            .by_ids(&order_ids)
            .await?
            .into_iter()
            .map(|order| (order.id, order))
            .collect();
        let pending: Vec<PaymentId> = feed
            .items
            .iter()
            .filter_map(|item| match &item.entry {
                ClientHistoryEntry::Payment(payment)
                    if payment.status == PaymentStatus::Pending =>
                {
                    Some(payment.id)
                }
                _ => None,
            })
            .collect();
        let mut reviews = conn.payments().charge_reviews(&pending).await?;
        let failed: Vec<PaymentId> = feed
            .items
            .iter()
            .filter_map(|item| match &item.entry {
                ClientHistoryEntry::Payment(payment) if payment.status == PaymentStatus::Failed => {
                    Some(payment.id)
                }
                _ => None,
            })
            .collect();
        let mut failures = conn.payments().failure_reasons(&failed).await?;
        let admins: HashMap<i64, String> = if feed.items.iter().any(|item| {
            matches!(&item.entry, ClientHistoryEntry::Audit(record) if record.actor_kind == "admin")
        }) {
            conn.admins()
                .list()
                .await?
                .into_iter()
                .map(|admin| (admin.id.0, admin.email))
                .collect()
        } else {
            HashMap::new()
        };
        let mut tickets: HashMap<TicketId, TicketBrief> = HashMap::new();
        for item in &feed.items {
            if let ClientHistoryEntry::Event(stored) = &item.entry
                && let DomainEvent::TicketOpened { ticket_id, .. }
                | DomainEvent::TicketAnswered { ticket_id, .. } = &stored.event
                && !tickets.contains_key(ticket_id)
            {
                match conn.tickets().get(*ticket_id).await {
                    Ok(ticket) => {
                        tickets.insert(
                            *ticket_id,
                            TicketBrief {
                                id: ticket.id,
                                state: ticket.filter(),
                                subject: ticket.subject,
                                status: ticket.status,
                            },
                        );
                    }
                    Err(StoreError::NotFound(_)) => {}
                    Err(err) => return Err(err.into()),
                }
            }
        }
        let items = feed
            .items
            .into_iter()
            .map(|item| {
                let record = match item.entry {
                    ClientHistoryEntry::Payment(payment) => HistoryRecord::Payment {
                        order: orders
                            .get(&payment.order_id)
                            .map(|order| order_brief(order, &tariffs)),
                        review: reviews.remove(&payment.id),
                        failure_reason: failures.remove(&payment.id),
                        payment: PaymentBrief {
                            id: payment.public_id,
                            number: payment.id,
                            status: payment.status,
                            amount: payment.amount,
                            provider: payment.provider,
                            method: payment.method,
                        },
                    },
                    ClientHistoryEntry::Order(order) => HistoryRecord::Order {
                        order: order_brief(&order, &tariffs),
                    },
                    ClientHistoryEntry::Event(stored) => {
                        let ticket = match &stored.event {
                            DomainEvent::TicketOpened { ticket_id, .. }
                            | DomainEvent::TicketAnswered { ticket_id, .. } => {
                                tickets.get(ticket_id).cloned()
                            }
                            _ => None,
                        };
                        HistoryRecord::Event {
                            kind: stored.event.kind().to_string(),
                            event: admin_event(stored.event),
                            ticket,
                        }
                    }
                    ClientHistoryEntry::Audit(record) => HistoryRecord::Audit {
                        actor_email: (record.actor_kind == "admin")
                            .then(|| record.actor_id.and_then(|id| admins.get(&id).cloned()))
                            .flatten(),
                        action: record.action,
                        actor_kind: record.actor_kind,
                        actor_id: record.actor_id,
                        entity_kind: record.entity_kind,
                        entity_id: record.entity_id,
                        details: record.details,
                    },
                };
                HistoryItem {
                    at: item.at,
                    record,
                }
            })
            .collect();
        Ok(HistoryPage {
            items,
            next: feed.next,
            total,
        })
    }

    pub async fn payment_status(
        &self,
        client_id: ClientId,
        payment: Uuid,
    ) -> ServiceResult<PaymentStatusView> {
        let mut conn = self.services.store().conn().await?;
        let payment = match conn.payments().by_public_id(payment).await {
            Ok(payment) if payment.client_id == client_id => payment,
            Ok(_) | Err(sn_store::StoreError::NotFound(_)) => {
                return Err(ServiceError::NotFound("платёж"));
            }
            Err(err) => return Err(err.into()),
        };
        let order = conn.orders().get(payment.order_id).await?;
        let pay_url =
            (payment.status == PaymentStatus::Pending && payment.pay_url.is_some()).then(|| {
                let base = self
                    .services
                    .settings()
                    .public_url
                    .clone()
                    .unwrap_or_default();
                format!(
                    "{}/api/pay/{}",
                    base.trim_end_matches('/'),
                    payment.public_id
                )
            });
        Ok(PaymentStatusView {
            payment_id: payment.public_id,
            status: payment.status,
            amount: payment.amount,
            provider: payment.provider,
            order_id: order.public_id,
            order_kind: order.kind,
            order_status: order.status,
            subscription_id: order.subscription_id,
            pay_url,
            expires_at: payment.expires_at,
            paid_at: payment.paid_at,
            created_at: payment.created_at,
        })
    }
}
