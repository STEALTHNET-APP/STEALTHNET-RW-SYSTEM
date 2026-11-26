use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::DomainError;
use sn_domain::access::ClientChannel;
use sn_domain::client::{Client, IdentityKind, proven_identities};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{
    CategoryId, ClientId, OrderId, PaymentId, PeriodId, SubscriptionId, TariffId,
};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Amount, Currency, Money, PayCurrency};
use sn_domain::order::{NewOrder, Order, OrderChannel, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::{NewPayment, Payment, PaymentStatus, ProviderId, providers};
use sn_domain::plan::{
    CurrentTariff, HeldSubscription, PlanKind, classify, new_expire_at, single_target,
};
use sn_domain::pricing::{Quote, QuoteRequest, quote, topup_quote};
use sn_domain::promotion::running_for;
use sn_domain::settings::{TrialVariant, keys};
use sn_domain::subscription::{Subscription, SubscriptionKind, SubscriptionStatus};
use sn_domain::tariff::{Category, PeriodOffer, Tariff, TariffSnapshot, period_offers};
use sn_modules::catalog::{antibot, promo};
use sn_modules::{ItemHook, ModuleContext, ModuleRegistry, QuoteHook, QuoteOrigin};
use sn_payments::{
    CheckoutMethod, ClientRef, InvoiceRequest, IssuedInvoice, MethodsContext, PaymentError,
    PaymentProvider, ProviderKind, ProviderRegistry, Unavailable,
};
use sn_store::{Db, InvoiceUpdate, LockSpace, StatusChange, StoreError, Tx, audit_entity};
use std::net::IpAddr;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(test)]
pub(crate) mod balance_off_tests;
#[cfg(test)]
mod channel_tests;
#[cfg(test)]
mod discount_tests;
#[cfg(test)]
mod gram_tests;
#[cfg(test)]
mod limit_tests;
#[cfg(test)]
mod method_tests;
#[cfg(test)]
mod order_channel_tests;
#[cfg(test)]
mod pool_tests;
#[cfg(test)]
mod promo_tests;
#[cfg(test)]
mod promotion_tests;
#[cfg(test)]
mod single_tests;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
mod topup;

pub use topup::{TopUpMethod, TopUpOptions};

const POLL_DELAY_SECONDS: i64 = 120;
const CHAIN_POLL_DELAY_SECONDS: i64 = 30;
const PROVIDER_CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartRequest {
    pub tariff_id: TariffId,
    pub period_id: PeriodId,
    #[serde(default)]
    pub extra_devices: u32,
    #[serde(default)]
    pub subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    pub promo_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Bot,
    Miniapp,
    Cabinet,
    Admin,
}

impl Channel {
    pub const fn client(self) -> Option<ClientChannel> {
        match self {
            Channel::Bot => Some(ClientChannel::Bot),
            Channel::Miniapp => Some(ClientChannel::Miniapp),
            Channel::Cabinet => Some(ClientChannel::Cabinet),
            Channel::Admin => None,
        }
    }

    pub const fn order(self) -> OrderChannel {
        match self {
            Channel::Bot => OrderChannel::Bot,
            Channel::Miniapp => OrderChannel::Miniapp,
            Channel::Cabinet => OrderChannel::Cabinet,
            Channel::Admin => OrderChannel::Admin,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutRequest {
    pub cart: CartRequest,
    pub provider: String,
    #[serde(default)]
    pub method: Option<String>,
    pub channel: Channel,
    #[serde(default)]
    pub return_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuoteView {
    pub quote: Quote,
    pub kind: PlanKind,
    pub expire_preview: DateTime<Utc>,
    pub methods: Vec<CheckoutMethod>,
    pub current: Option<CurrentSubscription>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promotion: Option<crate::promotions::PromotionNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentSubscription {
    pub subscription_id: SubscriptionId,
    pub tariff: Option<String>,
    pub trial: bool,
    pub expire_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckoutResult {
    Redirect {
        order_id: Uuid,
        payment_id: Uuid,
        pay_url: String,
    },
    Completed {
        order_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffOffer {
    pub category: Category,
    pub tariff: Tariff,
    pub periods: Vec<PeriodOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicTariff {
    pub id: TariffId,
    pub category_id: sn_domain::ids::CategoryId,
    pub name: String,
    pub description: Option<String>,
    pub devices: sn_domain::tariff::DevicePolicy,
    pub traffic: sn_domain::traffic::TrafficPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicOffer {
    pub category: Category,
    pub tariff: PublicTariff,
    pub periods: Vec<PeriodOffer>,
    pub countries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialChoice {
    pub id: u32,
    pub name: String,
    pub days: u32,
    pub devices: u32,
    pub traffic: sn_domain::traffic::TrafficPolicy,
    pub countries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialOffer {
    pub available: bool,
    pub variants: Vec<TrialChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRequest {
    pub module: ModuleId,
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub subscription_id: Option<SubscriptionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemCheckoutRequest {
    pub item: ItemRequest,
    pub provider: String,
    #[serde(default)]
    pub method: Option<String>,
    pub channel: Channel,
    #[serde(default)]
    pub return_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemQuoteView {
    pub quote: Quote,
    pub methods: Vec<CheckoutMethod>,
}

struct PayWith {
    provider: Arc<dyn PaymentProvider>,
    method: Option<String>,
}

struct Sale<'s> {
    order: &'s NewOrder,
    channel: OrderChannel,
    provider: &'s str,
    method: Option<String>,
    kind: ProviderKind,
    amount: Amount,
    description: String,
    return_url: Option<String>,
}

struct Basis {
    client: Client,
    tariff: Tariff,
    current: Option<Subscription>,
    current_tariff: Option<Tariff>,
    kind: PlanKind,
    balance: Money,
    telegram: bool,
}

struct Prepared {
    client: Client,
    tariff: Tariff,
    current: Option<Subscription>,
    current_tariff: Option<Tariff>,
    kind: PlanKind,
    quote: Quote,
    methods: Vec<CheckoutMethod>,
    promotion: Option<crate::promotions::PromotionNote>,
}

pub(crate) async fn held_subscriptions(
    db: &mut impl Db,
    live: &[Subscription],
) -> ServiceResult<Vec<HeldSubscription>> {
    let mut held = Vec::with_capacity(live.len());
    for subscription in live {
        let category_id = match subscription.tariff_id {
            Some(id) => match db.tariffs().get(id).await {
                Ok(tariff) => Some(tariff.category_id),
                Err(StoreError::NotFound(_)) => None,
                Err(err) => return Err(err.into()),
            },
            None => None,
        };
        held.push(HeldSubscription {
            id: subscription.id,
            category_id,
        });
    }
    Ok(held)
}

pub(crate) async fn single_subscription(
    db: &mut impl Db,
    client_id: ClientId,
    category: CategoryId,
) -> ServiceResult<Option<(Subscription, Option<CategoryId>)>> {
    let live: Vec<Subscription> = db
        .subscriptions()
        .by_client(client_id)
        .await?
        .into_iter()
        .filter(|s| s.ended_at.is_none())
        .collect();
    let held = held_subscriptions(db, &live).await?;
    let Some(id) = single_target(&held, category) else {
        return Ok(None);
    };
    let category = held.iter().find(|h| h.id == id).and_then(|h| h.category_id);
    Ok(live
        .into_iter()
        .find(|s| s.id == id)
        .map(|subscription| (subscription, category)))
}

pub const CATEGORY_FULL: &str = "subscription.category_full";
pub const CHOOSE_SUBSCRIPTION: &str = "subscription.choose";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryFull {
    pub category: Category,
    pub held: Vec<Subscription>,
}

impl CategoryFull {
    pub fn holds(&self, id: SubscriptionId) -> bool {
        self.held.iter().any(|s| s.id == id)
    }

    pub fn full_error(&self) -> ServiceError {
        ServiceError::rejected(
            CATEGORY_FULL,
            format!(
                "В категории «{}» новую подписку оформить нельзя: продлите действующую подписку этой категории или выберите тариф другой категории.",
                self.category.name
            ),
        )
    }

    pub fn choose_error(&self) -> ServiceError {
        ServiceError::rejected(
            CHOOSE_SUBSCRIPTION,
            format!(
                "Выберите, какую подписку категории «{}» продлить: новую в этой категории оформить нельзя.",
                self.category.name
            ),
        )
    }
}

pub(crate) async fn category_held(
    db: &mut impl Db,
    client_id: ClientId,
    category: CategoryId,
) -> ServiceResult<Vec<Subscription>> {
    let live: Vec<Subscription> = db
        .subscriptions()
        .by_client(client_id)
        .await?
        .into_iter()
        .filter(|s| s.ended_at.is_none())
        .collect();
    let held = held_subscriptions(db, &live).await?;
    let mut inside: Vec<Subscription> = live
        .into_iter()
        .filter(|s| {
            held.iter()
                .any(|h| h.id == s.id && h.category_id == Some(category))
        })
        .collect();
    inside.sort_by_key(|s| s.id);
    Ok(inside)
}

pub(crate) async fn category_full(
    db: &mut impl Db,
    multi: bool,
    client_id: ClientId,
    category_id: CategoryId,
) -> ServiceResult<Option<CategoryFull>> {
    if !multi {
        return Ok(None);
    }
    let category = match db.tariffs().category(category_id).await {
        Ok(category) => category,
        Err(StoreError::NotFound(_)) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if category.subscription_limit.is_none() {
        return Ok(None);
    }
    let held = category_held(db, client_id, category_id).await?;
    Ok(category
        .is_full(multi, held.len())
        .then_some(CategoryFull { category, held }))
}

pub(crate) struct PaidOrder {
    pub order: Order,
    pub payment: Payment,
    pub replaced: Vec<PaymentId>,
}

struct OrderHooks<'h> {
    modules: &'h ModuleRegistry,
    ctx: &'h ModuleContext,
}

pub(crate) async fn client_ref(db: &mut impl Db, client: &Client) -> ServiceResult<ClientRef> {
    let identities = db.clients().identities(client.id).await?;
    Ok(ClientRef {
        client_id: client.id,
        public_id: client.public_id,
        telegram_id: identities
            .iter()
            .find(|i| i.kind == IdentityKind::Telegram)
            .and_then(|i| i.value.parse().ok()),
        email: identities
            .iter()
            .find(|i| i.kind == IdentityKind::Email)
            .map(|i| i.value.clone()),
    })
}

pub(crate) async fn telegram_linked(db: &mut impl Db, client_id: ClientId) -> ServiceResult<bool> {
    Ok(db
        .clients()
        .identities(client_id)
        .await?
        .iter()
        .any(|i| i.kind == IdentityKind::Telegram))
}

pub(crate) fn reachable(
    registry: &ProviderRegistry,
    methods: Vec<CheckoutMethod>,
    telegram: bool,
) -> Vec<CheckoutMethod> {
    if telegram {
        return methods;
    }
    methods
        .into_iter()
        .filter(|m| registry.get(m.provider).is_none_or(|p| !p.needs_telegram()))
        .collect()
}

pub(crate) fn describe(brand: &str, quote: &Quote) -> String {
    match quote.lines.first() {
        Some(line) => format!("{brand}: {}", line.label),
        None => brand.to_string(),
    }
}

fn describe_for(brand: &str, quote: &Quote, provider: &str) -> String {
    match quote.lines.first() {
        Some(line) if provider == providers::STARS => format!("{brand}: {}", line.shown_label()),
        _ => describe(brand, quote),
    }
}

pub use sn_domain::payment::REPLACED_REASON;
pub const ORDER_REPLACED: &str = "order.replaced";

async fn detach_unpaid(
    modules: &ModuleRegistry,
    tx: &mut Tx,
    created: &Order,
) -> ServiceResult<Vec<OrderId>> {
    if !modules.is_enabled(ModuleId::Promo) {
        return Ok(Vec::new());
    }
    for order_id in promo::replacement_candidates(tx, created).await? {
        for payment in tx.payments().for_order(order_id).await? {
            if payment.status == PaymentStatus::Pending {
                tx.payments().lock(payment.id).await?;
            }
        }
        tx.orders().lock(order_id).await?;
    }
    let replaced = promo::superseded(tx, created).await?;
    promo::release_held(tx, &replaced).await?;
    Ok(replaced)
}

async fn replace_unpaid(
    modules: &ModuleRegistry,
    ctx: &ModuleContext,
    tx: &mut Tx,
    created: &Order,
) -> ServiceResult<Vec<PaymentId>> {
    let replaced = detach_unpaid(modules, tx, created).await?;
    cancel_replaced(modules, ctx, tx, created, &replaced).await
}

async fn cancel_replaced(
    modules: &ModuleRegistry,
    ctx: &ModuleContext,
    tx: &mut Tx,
    created: &Order,
    replaced: &[OrderId],
) -> ServiceResult<Vec<PaymentId>> {
    let code = promo::promo_in_quote(&created.quote).map(|(_, code)| code);
    let mut dropped = Vec::new();
    for &order_id in replaced {
        let mut payments = Vec::new();
        for payment in tx.payments().for_order(order_id).await? {
            if payment.status != PaymentStatus::Pending {
                continue;
            }
            match tx
                .payments()
                .transition(
                    payment.id,
                    PaymentStatus::Canceled,
                    None,
                    Some(REPLACED_REASON),
                )
                .await
            {
                Ok(change) if change.applied() => payments.push(payment.id),
                Ok(_) | Err(StoreError::Invalid(_)) => {}
                Err(err) => return Err(err.into()),
            }
        }
        dropped.extend(payments.iter().copied());
        let canceled = match tx
            .orders()
            .transition(order_id, OrderStatus::Canceled)
            .await
        {
            Ok((canceled, true)) => canceled,
            Ok(_) | Err(StoreError::Invalid(_)) => continue,
            Err(err) => return Err(err.into()),
        };
        modules.order_canceled(ctx, tx, &canceled).await?;
        tx.audit()
            .record(&Actor::Client(created.client_id).audit(
                ORDER_REPLACED,
                audit_entity::ORDER,
                Some(order_id.0),
                serde_json::json!({
                    "replaced_by": created.id,
                    "promo": code,
                    "canceled_payments": payments,
                }),
            ))
            .await?;
        tracing::info!(order = %order_id, by = %created.id, "неоплаченный счёт отменён, промокод перенесён в новый счёт");
    }
    Ok(dropped)
}

type InvoiceRow = (String, Option<String>, Option<serde_json::Value>);

pub(crate) async fn cancel_at_providers(services: &Services, payments: &[PaymentId]) {
    for &payment_id in payments {
        let found: ServiceResult<Option<InvoiceRow>> = async {
            let mut conn = services.store().conn().await?;
            Ok(sqlx::query_as(
                "SELECT provider, provider_txid, provider_payload FROM payments WHERE id = $1",
            )
            .bind(payment_id.0)
            .fetch_optional(conn.conn())
            .await?)
        }
        .await;
        let (provider_id, txid, payload) = match found {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!(payment = %payment_id, error = %err, "заменённый счёт не прочитан для отмены в кассе");
                continue;
            }
        };
        let Some(provider) = services.payments().get(&provider_id) else {
            continue;
        };
        let payload = payload.unwrap_or(serde_json::Value::Null);
        let config = services.payments().config(provider.id());
        let canceled = tokio::time::timeout(
            PROVIDER_CANCEL_TIMEOUT,
            provider.cancel_invoice(
                &config,
                &IssuedInvoice {
                    external_id: txid.as_deref(),
                    payload: &payload,
                },
            ),
        )
        .await;
        match canceled {
            Ok(Ok(())) => {
                tracing::info!(payment = %payment_id, provider = provider.id(), "заменённый счёт отменён и в платёжной системе");
            }
            Ok(Err(PaymentError::Unsupported(_))) => {}
            Ok(Err(err)) => {
                tracing::warn!(payment = %payment_id, provider = provider.id(), error = %err, "заменённый счёт не отменился в платёжной системе");
            }
            Err(_) => {
                tracing::warn!(payment = %payment_id, provider = provider.id(), "платёжная система не ответила на отмену заменённого счёта");
            }
        }
    }
}

pub(crate) async fn record_paid_order(
    tx: &mut Tx,
    order: &NewOrder,
    channel: OrderChannel,
    provider: &str,
    method: Option<&str>,
    amount: Amount,
    debit: Option<LedgerKind>,
) -> ServiceResult<PaidOrder> {
    record_paid(tx, order, channel, provider, method, amount, debit, None).await
}

#[allow(clippy::too_many_arguments)]
async fn record_paid(
    tx: &mut Tx,
    order: &NewOrder,
    channel: OrderChannel,
    provider: &str,
    method: Option<&str>,
    amount: Amount,
    debit: Option<LedgerKind>,
    hooks: Option<&OrderHooks<'_>>,
) -> ServiceResult<PaidOrder> {
    if order.kind == OrderKind::TopUp {
        return Err(ServiceError::rejected(
            "checkout.topup_internal",
            "пополнение оплачивается только через платёжную систему",
        ));
    }
    let created = tx.orders().create(order).await?;
    tx.orders().set_channel(created.id, channel).await?;
    let payment = tx
        .payments()
        .create(&NewPayment {
            order_id: created.id,
            client_id: created.client_id,
            provider: ProviderId::from(provider),
            method: method.map(str::to_string),
            amount,
            expires_at: None,
        })
        .await?;
    let mut replaced = Vec::new();
    if let Some(hooks) = hooks {
        replaced = replace_unpaid(hooks.modules, hooks.ctx, tx, &created).await?;
        hooks
            .modules
            .order_created(hooks.ctx, tx, &created, &payment)
            .await?;
    }
    if let Some(kind) = debit {
        let money = amount.as_money().ok_or_else(|| {
            ServiceError::rejected(
                "checkout.balance_currency",
                "с баланса оплачивается только сумма в валюте",
            )
        })?;
        let delta = money.checked_neg()?;
        let prefix = match kind {
            LedgerKind::AutoRenew => "autorenew",
            _ => "purchase",
        };
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: created.client_id,
                wallet: Wallet::Main,
                amount: delta,
                kind,
                order_id: Some(created.id),
                payment_id: Some(payment.id),
                admin_id: None,
                comment: None,
                idempotency_key: Some(format!("{prefix}:order:{}", created.id)),
            })
            .await
            .map_err(|err| match err {
                StoreError::InsufficientFunds => ServiceError::InsufficientFunds,
                other => other.into(),
            })?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: created.client_id,
                    wallet: Wallet::Main,
                    kind,
                    delta,
                })
                .await?;
        }
    }
    let paid = match tx
        .payments()
        .transition(payment.id, PaymentStatus::Paid, None, None)
        .await?
    {
        StatusChange::Applied(p) | StatusChange::Unchanged(p) => p,
    };
    let (order, _) = tx
        .orders()
        .transition(created.id, OrderStatus::Paid)
        .await?;
    if let Some(hooks) = hooks {
        hooks
            .modules
            .order_paid(hooks.ctx, tx, &order, &paid)
            .await?;
    }
    tx.jobs()
        .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
        .await?;
    tx.events()
        .push(&DomainEvent::PaymentPaid {
            payment_id: paid.id,
            order_id: order.id,
            client_id: order.client_id,
            order_kind: order.kind,
            amount: paid.amount,
            provider: paid.provider.clone(),
        })
        .await?;
    Ok(PaidOrder {
        order,
        payment: paid,
        replaced,
    })
}

pub const BALANCE_OFF: &str = "balance.disabled";
pub const GRAM_WALLET_OFF: &str = "gram.wallet_unavailable";
pub const GRAM_WIDGET_TEMPLATE: &str = "gram.widget";

fn pick_method(
    methods: &[CheckoutMethod],
    provider: &str,
    code: Option<&str>,
    balance_on: bool,
) -> ServiceResult<(ProviderKind, Amount, Option<String>)> {
    let code = code.map(str::trim).filter(|c| !c.is_empty());
    let method = methods
        .iter()
        .find(|m| m.provider == provider && (code.is_none() || m.method.as_deref() == code))
        .ok_or_else(|| {
            if provider == providers::BALANCE && !balance_on {
                ServiceError::rejected(
                    BALANCE_OFF,
                    "Оплата с баланса выключена: остатком можно оплатить, только если его хватает на всю сумму. Выберите другой способ оплаты.",
                )
            } else {
                ServiceError::rejected(
                    "checkout.method_unavailable",
                    "Этот способ оплаты сейчас недоступен",
                )
            }
        })?;
    if let Some(reason) = &method.unavailable {
        return Err(ServiceError::rejected(
            "checkout.method_unavailable",
            match reason {
                Unavailable::InsufficientBalance { balance, .. } => {
                    format!("Не хватает: на балансе {}", format::money(*balance))
                }
                Unavailable::NoStarsPrice => "У этого срока нет цены в звёздах".to_string(),
            },
        ));
    }
    let amount = method.amount.ok_or_else(|| {
        ServiceError::rejected(
            "checkout.method_unavailable",
            "Этот способ оплаты сейчас недоступен",
        )
    })?;
    Ok((method.kind, amount, method.method.clone()))
}

fn trusted_return_url(url: &str, public_url: Option<&str>) -> bool {
    let same_origin = |base: &str| {
        url == base
            || url
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('?'))
    };
    url.starts_with("https://")
        && (public_url.is_some_and(same_origin) || same_origin("https://t.me"))
}

pub fn antibot_refusal(outcome: antibot::Confirmation) -> ServiceError {
    match outcome {
        antibot::Confirmation::TooFast => {
            ServiceError::rejected(antibot::TOO_FAST, "Нажмите кнопку ещё раз")
        }
        antibot::Confirmation::Failed => {
            ServiceError::rejected(antibot::FAILED, "Проверка не пройдена. Попробуйте ещё раз")
        }
        antibot::Confirmation::Unavailable => ServiceError::rejected(
            antibot::UNAVAILABLE,
            "Не удалось проверить, что вы не робот: сервис проверки не ответил. Попробуйте ещё раз через минуту",
        ),
        antibot::Confirmation::TooMany => crate::limits::rate_limited(
            "Слишком много попыток.",
            Duration::seconds(antibot::checks::ANSWERS.window_seconds),
        ),
        antibot::Confirmation::Expired | antibot::Confirmation::Confirmed => {
            ServiceError::rejected(
                antibot::EXPIRED,
                "Проверка устарела. Запросите пробный период заново",
            )
        }
    }
}

pub(crate) const TRIAL_NAME: &str = "Пробный период";

pub(crate) fn trial_snapshot(variant: &TrialVariant) -> TariffSnapshot {
    TariffSnapshot {
        traffic_leftover: Default::default(),
        tariff_id: TariffId(0),
        category_id: CategoryId(0),
        name: if variant.name.is_empty() {
            TRIAL_NAME.into()
        } else {
            variant.name.clone()
        },
        period_id: PeriodId(0),
        days: variant.days,
        devices_included: variant.devices,
        max_extra_devices: 0,
        traffic: variant.traffic,
        squads: variant.squads.clone(),
        external_squad: None,
    }
}

pub struct Checkout<'a> {
    services: &'a Services,
}

impl<'a> Checkout<'a> {
    pub fn new(services: &'a Services) -> Self {
        Checkout { services }
    }

    pub async fn catalog(&self) -> ServiceResult<Vec<TariffOffer>> {
        self.catalog_in(None).await
    }

    pub async fn public_catalog(&self) -> ServiceResult<Vec<PublicOffer>> {
        let offers = self
            .catalog_in(Some(sn_domain::access::ClientChannel::Cabinet))
            .await?;
        let mut squads: Vec<Uuid> = Vec::new();
        for offer in &offers {
            for squad in &offer.tariff.squads {
                if !squads.contains(&squad.0) {
                    squads.push(squad.0);
                }
            }
        }
        let countries = self.services.squad_countries(&squads).await;
        Ok(offers
            .into_iter()
            .map(|offer| {
                let squads: Vec<Uuid> = offer.tariff.squads.iter().map(|s| s.0).collect();
                let codes = crate::squad_countries::union(&squads, &countries);
                PublicOffer {
                    category: offer.category,
                    tariff: PublicTariff {
                        id: offer.tariff.id,
                        category_id: offer.tariff.category_id,
                        name: offer.tariff.name,
                        description: offer.tariff.description,
                        devices: offer.tariff.devices,
                        traffic: offer.tariff.traffic,
                    },
                    periods: offer.periods,
                    countries: codes,
                }
            })
            .collect())
    }

    pub async fn catalog_in(
        &self,
        channel: Option<sn_domain::access::ClientChannel>,
    ) -> ServiceResult<Vec<TariffOffer>> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        let categories = conn.tariffs().categories(false).await?;
        let tariffs = conn.tariffs().list(false).await?;
        let ids: Vec<TariffId> = tariffs.iter().map(|t| t.id).collect();
        let channels = conn.tariffs().channels(&ids).await?;
        let now = self.services.now();
        let running = crate::promotions::running(&mut conn, now).await?;
        let mut offers = Vec::new();
        for category in categories {
            for tariff in tariffs.iter().filter(|t| t.category_id == category.id) {
                if !tariff.is_offered(currency) {
                    continue;
                }
                if let Some(channel) = channel
                    && !channels
                        .get(&tariff.id)
                        .copied()
                        .unwrap_or_default()
                        .allows(channel)
                {
                    continue;
                }
                let promotion = running_for(&running, tariff.id, now);
                let mut periods = period_offers(tariff, currency, promotion.as_ref());
                crate::promotions::label_offers(self.services, &mut periods);
                offers.push(TariffOffer {
                    category: category.clone(),
                    tariff: tariff.clone(),
                    periods,
                });
            }
        }
        Ok(offers)
    }

    async fn basis(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        currency: Currency,
    ) -> ServiceResult<Basis> {
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let tariff = conn.tariffs().get(cart.tariff_id).await?;
        let multi = self.services.settings().multi_subscriptions;
        let full = category_full(&mut conn, multi, client_id, tariff.category_id).await?;
        let current = match cart.subscription_id {
            Some(id) => {
                let subscription = conn.subscriptions().get(id).await?;
                if subscription.client_id != client_id {
                    return Err(ServiceError::NotFound("подписка"));
                }
                if let Some(full) = &full
                    && !full.holds(id)
                {
                    return Err(full.full_error());
                }
                Some(subscription)
            }
            None if multi => match &full {
                Some(full) => match full.held.as_slice() {
                    [only] => Some(only.clone()),
                    _ => return Err(full.choose_error()),
                },
                None => conn
                    .subscriptions()
                    .by_client(client_id)
                    .await?
                    .into_iter()
                    .find(|s| s.kind == SubscriptionKind::Trial && s.ended_at.is_none()),
            },
            None => single_subscription(&mut conn, client_id, tariff.category_id)
                .await?
                .map(|(subscription, _)| subscription),
        };
        if current
            .as_ref()
            .is_some_and(|s| s.mirror.status == Some(SubscriptionStatus::Disabled))
        {
            return Err(ServiceError::rejected(
                "subscription.disabled",
                "Подписка заблокирована, оплата недоступна. Напишите в поддержку.",
            ));
        }
        let current_tariff = match current.as_ref().and_then(|s| s.tariff_id) {
            Some(id) => conn.tariffs().get(id).await.ok(),
            None => None,
        };
        let kind = classify(
            tariff.category_id,
            current.as_ref().map(|s| CurrentTariff {
                kind: s.kind,
                category_id: current_tariff.as_ref().map(|t| t.category_id),
            }),
        );
        let balance = conn
            .ledger()
            .balance(client_id, Wallet::Main, currency)
            .await?;
        let telegram = telegram_linked(&mut conn, client_id).await?;
        Ok(Basis {
            client,
            tariff,
            current,
            current_tariff,
            kind,
            balance,
            telegram,
        })
    }

    async fn prepare(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        origin: &QuoteOrigin,
        channel: Option<ClientChannel>,
        promoted: bool,
    ) -> ServiceResult<Prepared> {
        let currency = self.services.settings().currency;
        let modules = self.services.modules();
        if cart.promo_code.is_some() && !modules.is_enabled(ModuleId::Promo) {
            return Err(ServiceError::rejected(
                "promo.disabled",
                "Промокоды сейчас не действуют",
            ));
        }
        let Basis {
            client,
            tariff,
            current,
            current_tariff,
            kind,
            balance,
            telegram,
        } = self.basis(client_id, cart, currency).await?;
        let hook = QuoteHook {
            client_id,
            tariff_id: tariff.id,
            period_id: cart.period_id,
            order_kind: kind.order_kind(),
            currency,
            promo_code: cart.promo_code.clone(),
        };
        if cart.extra_devices > 0
            && let Some(current) = current.as_ref()
            && current.tariff_id == Some(tariff.id)
        {
            let mut conn = self.services.store().conn().await?;
            if crate::renewals::live_link(&mut conn, current.id)
                .await?
                .is_some()
            {
                return Err(crate::renewals::devices_refusal());
            }
        }
        let adjustments = modules
            .quote_adjustments_from(&self.services.module_context(), &hook, origin)
            .await?;
        let promotion = if promoted {
            let mut conn = self.services.store().conn().await?;
            crate::promotions::running_terms(&mut conn, tariff.id, self.services.now()).await?
        } else {
            None
        };
        let quote = quote(&QuoteRequest {
            currency,
            tariff: &tariff,
            period_id: cart.period_id,
            extra_devices: cart.extra_devices,
            sell_extra_devices: modules.is_enabled(ModuleId::Options),
            adjustments: &adjustments,
            promotion: promotion.as_ref(),
        })?;
        let promotion = crate::promotions::note(
            self.services,
            promotion.as_ref(),
            &quote,
            !adjustments.is_empty(),
        );
        let methods = reachable(
            self.services.payments(),
            self.services.payments().checkout_methods(&MethodsContext {
                currency,
                total: quote.total,
                stars_total: quote.stars.map(|s| s.total),
                balance: Some(balance),
                channel,
            }),
            telegram,
        );
        let methods = self
            .services
            .renewals()
            .offer_methods(
                methods,
                &crate::renewals::RenewalCart {
                    tariff: &tariff,
                    period: cart.period_id,
                    quote: &quote,
                    currency,
                    extra_devices: cart.extra_devices,
                    current: current.as_ref(),
                },
            )
            .await?;
        Ok(Prepared {
            client,
            tariff,
            current,
            current_tariff,
            kind,
            quote,
            methods,
            promotion,
        })
    }

    pub async fn category_room(
        &self,
        client_id: ClientId,
        tariff_id: TariffId,
    ) -> ServiceResult<Option<CategoryFull>> {
        let mut conn = self.services.store().conn().await?;
        let tariff = conn.tariffs().get(tariff_id).await?;
        category_full(
            &mut conn,
            self.services.settings().multi_subscriptions,
            client_id,
            tariff.category_id,
        )
        .await
    }

    pub async fn quote(&self, client_id: ClientId, cart: &CartRequest) -> ServiceResult<QuoteView> {
        self.quote_for(client_id, cart, None, None).await
    }

    pub async fn quote_in(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        channel: Channel,
    ) -> ServiceResult<QuoteView> {
        self.quote_for(client_id, cart, None, channel.client())
            .await
    }

    pub async fn quote_from(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        address: Option<IpAddr>,
    ) -> ServiceResult<QuoteView> {
        self.quote_for(client_id, cart, address, None).await
    }

    pub async fn quote_for(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
        address: Option<IpAddr>,
        channel: Option<ClientChannel>,
    ) -> ServiceResult<QuoteView> {
        let prepared = self
            .prepare(client_id, cart, &QuoteOrigin { address }, channel, true)
            .await?;
        let expire_preview = new_expire_at(
            prepared.kind,
            prepared.current.as_ref().and_then(|s| s.mirror.expire_at),
            self.services.now(),
            prepared.quote.days,
        );
        let current = prepared.current.as_ref().map(|s| CurrentSubscription {
            subscription_id: s.id,
            tariff: prepared.current_tariff.as_ref().map(|t| t.name.clone()),
            trial: s.kind == SubscriptionKind::Trial,
            expire_at: s.mirror.expire_at,
        });
        Ok(QuoteView {
            quote: prepared.quote,
            kind: prepared.kind,
            expire_preview,
            methods: prepared.methods,
            current,
            promotion: prepared.promotion,
        })
    }

    pub async fn start(
        &self,
        client_id: ClientId,
        request: &CheckoutRequest,
    ) -> ServiceResult<CheckoutResult> {
        self.start_from(client_id, request, None).await
    }

    pub async fn start_from(
        &self,
        client_id: ClientId,
        request: &CheckoutRequest,
        address: Option<IpAddr>,
    ) -> ServiceResult<CheckoutResult> {
        self.services.remna()?;
        let prepared = self
            .prepare(
                client_id,
                &request.cart,
                &QuoteOrigin { address },
                request.channel.client(),
                true,
            )
            .await?;
        let (kind, amount, method) = pick_method(
            &prepared.methods,
            &request.provider,
            request.method.as_deref(),
            self.services.payments().balance_enabled(),
        )?;
        let period = prepared
            .tariff
            .period(request.cart.period_id)
            .ok_or(DomainError::PeriodNotFound(request.cart.period_id))?;
        let target = prepared.current.as_ref().map(|s| s.id);
        let order = NewOrder {
            client_id,
            subscription_id: target,
            kind: prepared.kind.order_kind(),
            intent: PurchaseIntent::Subscription {
                tariff_id: prepared.tariff.id,
                period_id: period.id,
                extra_devices: request.cart.extra_devices,
                target,
            },
            quote: prepared.quote.clone(),
            snapshot: Some(prepared.tariff.snapshot(period)),
        };
        let description = describe_for(
            &self.services.settings().brand_name,
            &prepared.quote,
            &request.provider,
        );
        self.settle(
            &prepared.client,
            Sale {
                order: &order,
                channel: request.channel.order(),
                provider: &request.provider,
                method,
                kind,
                amount,
                description,
                return_url: request.return_url.clone(),
            },
        )
        .await
    }

    pub async fn gram_in_telegram(
        &self,
        client_id: ClientId,
        payment: Uuid,
    ) -> ServiceResult<String> {
        let unavailable =
            |message: &str| ServiceError::rejected(GRAM_WALLET_OFF, message.to_string());
        if self
            .services
            .payments()
            .config(providers::GRAM)
            .flag("testnet")
        {
            return Err(unavailable(
                "Кошелёк Telegram не работает с тестовой сетью TON. Оплатите другим кошельком.",
            ));
        }
        if self.services.bot_token().is_none() {
            return Err(unavailable("Бот не подключён. Оплатите другим кошельком."));
        }
        let mut tx = self.services.store().begin().await?;
        let username = tx
            .settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| unavailable("Бот не подключён. Оплатите другим кошельком."))?;
        let found = match tx.payments().by_public_id(payment).await {
            Ok(found)
                if found.client_id == client_id && found.provider.as_str() == providers::GRAM =>
            {
                found
            }
            Ok(_) | Err(StoreError::NotFound(_)) => return Err(ServiceError::NotFound("счёт")),
            Err(err) => return Err(err.into()),
        };
        if found.status != PaymentStatus::Pending
            || found.expires_at.is_some_and(|at| at <= self.services.now())
        {
            return Err(ServiceError::rejected(
                "gram.closed",
                "Счёт уже закрыт. Оформите оплату заново.",
            ));
        }
        let client = tx.clients().get(client_id).await?;
        let telegram = tx
            .clients()
            .identities(client_id)
            .await?
            .into_iter()
            .any(|identity| identity.kind == IdentityKind::Telegram);
        if !telegram || client.bot_unreachable {
            return Err(unavailable(
                "Бот не может написать вам. Откройте бота в Telegram, нажмите «Старт» и попробуйте ещё раз.",
            ));
        }
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id,
                    template: GRAM_WIDGET_TEMPLATE.into(),
                    params: serde_json::json!({ "payment_id": found.public_id }),
                },
                None,
            )
            .await?;
        tx.commit().await?;
        Ok(format!("https://t.me/{}", username.trim_start_matches('@')))
    }

    pub fn topup_open(&self) -> ServiceResult<()> {
        if self.services.payments().balance_enabled() {
            Ok(())
        } else {
            Err(ServiceError::rejected(
                BALANCE_OFF,
                "Пополнение баланса выключено. Оплатите подписку напрямую одним из способов оплаты.",
            ))
        }
    }

    pub async fn topup_from(
        &self,
        client_id: ClientId,
        amount: Money,
        provider: &str,
        method: Option<&str>,
        channel: Channel,
        return_url: Option<String>,
    ) -> ServiceResult<CheckoutResult> {
        self.topup_open()?;
        let settings = self.services.settings();
        if amount.currency != settings.currency {
            return Err(ServiceError::rejected(
                "topup.currency",
                format!("Баланс пополняется в {}", settings.currency),
            ));
        }
        topup_quote(amount, settings.topup_limits())?;
        let (client, telegram) = {
            let mut conn = self.services.store().conn().await?;
            let client = conn.clients().get(client_id).await?;
            let telegram = telegram_linked(&mut conn, client_id).await?;
            (client, telegram)
        };
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let registry = self.services.payments();
        let code = method.map(str::trim).filter(|c| !c.is_empty());
        let methods = sn_payments::plain_methods(registry.checkout_methods(&MethodsContext {
            currency: amount.currency,
            total: amount,
            stars_total: None,
            balance: None,
            channel: channel.client(),
        }));
        let chosen = reachable(registry, methods, telegram)
            .into_iter()
            .find(|m| {
                m.kind == ProviderKind::External
                    && m.unavailable.is_none()
                    && m.provider == provider
                    && (code.is_none() || m.method.as_deref() == code)
            });
        let (provider, method, charged) = chosen
            .and_then(|m| {
                let charged = m
                    .amount
                    .and_then(Amount::as_money)
                    .filter(|charged| charged.currency == amount.currency)
                    .unwrap_or(amount);
                registry.get(m.provider).map(|p| (p, m.method, charged))
            })
            .ok_or_else(|| {
                ServiceError::rejected(
                    "checkout.method_unavailable",
                    "Пополнить баланс этим способом нельзя",
                )
            })?;
        let amount = charged;
        let quote = topup_quote(amount, settings.topup_limits())?;
        let order = NewOrder {
            client_id,
            subscription_id: None,
            kind: OrderKind::TopUp,
            intent: PurchaseIntent::TopUp { amount },
            quote: quote.clone(),
            snapshot: None,
        };
        let description = describe(&settings.brand_name, &quote);
        self.invoice(
            &client,
            &order,
            channel.order(),
            PayWith { provider, method },
            amount.into(),
            description,
            return_url,
        )
        .await
    }

    pub(crate) async fn plan_order(
        &self,
        client_id: ClientId,
        cart: &CartRequest,
    ) -> ServiceResult<NewOrder> {
        let prepared = self
            .prepare(client_id, cart, &QuoteOrigin { address: None }, None, false)
            .await?;
        let period = prepared
            .tariff
            .period(cart.period_id)
            .ok_or(DomainError::PeriodNotFound(cart.period_id))?;
        let target = prepared.current.as_ref().map(|s| s.id);
        Ok(NewOrder {
            client_id,
            subscription_id: target,
            kind: prepared.kind.order_kind(),
            intent: PurchaseIntent::Subscription {
                tariff_id: prepared.tariff.id,
                period_id: period.id,
                extra_devices: cart.extra_devices,
                target,
            },
            quote: prepared.quote.clone(),
            snapshot: Some(prepared.tariff.snapshot(period)),
        })
    }

    pub async fn start_trial(
        &self,
        client_id: ClientId,
        variant: Option<u32>,
    ) -> ServiceResult<OrderId> {
        self.services.remna()?;
        let settings = self.services.settings();
        let trial = &settings.trial;
        if !trial.enabled {
            return Err(ServiceError::rejected(
                "trial.disabled",
                "Пробный период сейчас недоступен",
            ));
        }
        let offered = trial.offered();
        let variant = match (variant, offered.as_slice()) {
            (_, []) => {
                return Err(ServiceError::rejected(
                    "trial.not_configured",
                    "Пробный период не настроен: укажите срок и сквады",
                ));
            }
            (Some(id), _) => offered
                .iter()
                .find(|v| v.id == id)
                .copied()
                .ok_or_else(|| {
                    ServiceError::rejected(
                        "trial.variant_unavailable",
                        "Этот вариант пробного периода сейчас недоступен, выберите другой",
                    )
                })?,
            (None, [only]) => *only,
            (None, _) => {
                return Err(ServiceError::rejected(
                    "trial.variant_required",
                    "Выберите вариант пробного периода",
                ));
            }
        };
        self.trial_guard(client_id).await?;
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::Client, client_id.0).await?;
        let client = tx.clients().get(client_id).await?;
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let subscriptions = tx.subscriptions().by_client(client_id).await?;
        if subscriptions.iter().any(|s| s.ended_at.is_none()) {
            return Err(ServiceError::rejected(
                "trial.has_subscription",
                "Пробный период доступен только до первой подписки",
            ));
        }
        let identities = proven_identities(tx.clients().identities(client_id).await?);
        if identities.is_empty() {
            return Err(ServiceError::rejected(
                "trial.no_identity",
                "Пробный период выдаётся только аккаунту с Telegram или почтой",
            ));
        }
        if !tx.trials().claim(client_id, &identities).await? {
            return Err(ServiceError::rejected(
                "trial.used",
                "Пробный период уже был использован",
            ));
        }
        let currency = settings.currency;
        let order = tx
            .orders()
            .create(&NewOrder {
                client_id,
                subscription_id: None,
                kind: OrderKind::Trial,
                intent: PurchaseIntent::Trial,
                quote: Quote {
                    currency,
                    lines: Vec::new(),
                    subtotal: Money::zero(currency),
                    discount: Money::zero(currency),
                    total: Money::zero(currency),
                    stars: None,
                    days: variant.days,
                    extra_devices: 0,
                },
                snapshot: Some(trial_snapshot(variant)),
            })
            .await?;
        tx.orders().transition(order.id, OrderStatus::Paid).await?;
        tx.jobs()
            .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
            .await?;
        tx.commit().await?;
        Ok(order.id)
    }

    async fn trial_guard(&self, client_id: ClientId) -> ServiceResult<()> {
        {
            let mut conn = self.services.store().conn().await?;
            let live = conn
                .subscriptions()
                .by_client(client_id)
                .await?
                .iter()
                .any(|s| s.ended_at.is_none());
            let identities = proven_identities(conn.clients().identities(client_id).await?);
            if live
                || identities.is_empty()
                || conn.trials().was_claimed(client_id, &identities).await?
            {
                return Ok(());
            }
        }
        match antibot::trial_gate(&self.services.module_context(), client_id).await? {
            antibot::Gate::Allow => Ok(()),
            antibot::Gate::Confirm => Err(ServiceError::rejected(
                antibot::CONFIRM_CODE,
                "Подтвердите, что вы не робот, и пробный период включится",
            )),
            antibot::Gate::Deny => Err(ServiceError::rejected(
                antibot::DENIED_CODE,
                "Пробный период для этого аккаунта недоступен. Если это ошибка, напишите в поддержку",
            )),
        }
    }

    pub async fn trial_open(&self, client_id: ClientId) -> ServiceResult<bool> {
        if !self.services.settings().trial.is_offered() {
            return Ok(false);
        }
        let mut conn = self.services.store().conn().await?;
        if conn.clients().get(client_id).await?.is_blocked() {
            return Ok(false);
        }
        let live = conn
            .subscriptions()
            .by_client(client_id)
            .await?
            .iter()
            .any(|s| s.ended_at.is_none());
        if live {
            return Ok(false);
        }
        let identities = proven_identities(conn.clients().identities(client_id).await?);
        Ok(!identities.is_empty() && !conn.trials().was_claimed(client_id, &identities).await?)
    }

    pub async fn trial_offer(&self, client_id: ClientId) -> ServiceResult<TrialOffer> {
        let offered: Vec<TrialVariant> = self
            .services
            .settings()
            .trial
            .offered()
            .into_iter()
            .cloned()
            .collect();
        if offered.is_empty() || !self.trial_open(client_id).await? {
            return Ok(TrialOffer {
                available: false,
                variants: Vec::new(),
            });
        }
        let mut squads: Vec<Uuid> = Vec::new();
        for squad in offered.iter().flat_map(|v| v.squads.iter()) {
            if !squads.contains(&squad.0) {
                squads.push(squad.0);
            }
        }
        let countries = self.services.squad_countries(&squads).await;
        Ok(TrialOffer {
            available: true,
            variants: offered
                .into_iter()
                .map(|v| {
                    let own: Vec<Uuid> = v.squads.iter().map(|s| s.0).collect();
                    TrialChoice {
                        id: v.id,
                        countries: crate::squad_countries::union(&own, &countries),
                        name: v.name,
                        days: v.days,
                        devices: v.devices,
                        traffic: v.traffic,
                    }
                })
                .collect(),
        })
    }

    pub async fn trial_challenge(
        &self,
        client_id: ClientId,
        surface: antibot::Surface,
        variant: Option<u32>,
    ) -> ServiceResult<antibot::Challenge> {
        Ok(
            antibot::trial_challenge(&self.services.module_context(), client_id, surface, variant)
                .await?,
        )
    }

    pub async fn confirm_trial(
        &self,
        client_id: ClientId,
        token: &str,
        answer: antibot::Answer<'_>,
        variant: Option<u32>,
    ) -> ServiceResult<OrderId> {
        let ctx = self.services.module_context();
        match antibot::confirm(&ctx, client_id, token, antibot::TRIAL, answer).await? {
            antibot::Confirmation::Confirmed => self.start_trial(client_id, variant).await,
            other => Err(antibot_refusal(other)),
        }
    }

    async fn prepare_item(
        &self,
        client_id: ClientId,
        item: &ItemRequest,
        channel: Option<ClientChannel>,
    ) -> ServiceResult<(Client, Quote, Vec<CheckoutMethod>)> {
        let currency = self.services.settings().currency;
        let modules = self.services.modules();
        if !modules.is_enabled(item.module) {
            return Err(sn_modules::ModuleError::Disabled(item.module).into());
        }
        let (client, balance, telegram) = {
            let mut conn = self.services.store().conn().await?;
            let client = conn.clients().get(client_id).await?;
            if client.is_blocked() {
                return Err(ServiceError::Forbidden);
            }
            if let Some(id) = item.subscription_id {
                let subscription = conn.subscriptions().get(id).await?;
                if subscription.client_id != client_id {
                    return Err(ServiceError::NotFound("подписка"));
                }
                if item.module == ModuleId::Options
                    && item.kind == sn_modules::catalog::options::ITEM_DEVICES
                    && crate::renewals::live_link(&mut conn, id).await?.is_some()
                {
                    return Err(crate::renewals::devices_refusal());
                }
            }
            let balance = conn
                .ledger()
                .balance(client_id, Wallet::Main, currency)
                .await?;
            let telegram = telegram_linked(&mut conn, client_id).await?;
            (client, balance, telegram)
        };
        let quote = modules
            .get(item.module)
            .quote_item(
                &self.services.module_context(),
                &ItemHook {
                    client_id,
                    currency,
                    kind: item.kind.clone(),
                    payload: item.payload.clone(),
                },
            )
            .await?;
        if quote.currency != currency || quote.total.is_negative() {
            return Err(ServiceError::rejected(
                "checkout.item_price",
                "модуль посчитал цену не в валюте установки",
            ));
        }
        let methods = reachable(
            self.services.payments(),
            sn_payments::plain_methods(self.services.payments().checkout_methods(
                &MethodsContext {
                    currency,
                    total: quote.total,
                    stars_total: quote.stars.map(|s| s.total),
                    balance: Some(balance),
                    channel,
                },
            )),
            telegram,
        );
        Ok((client, quote, methods))
    }

    pub async fn quote_item(
        &self,
        client_id: ClientId,
        item: &ItemRequest,
    ) -> ServiceResult<ItemQuoteView> {
        self.quote_item_in(client_id, item, None).await
    }

    pub async fn quote_item_in(
        &self,
        client_id: ClientId,
        item: &ItemRequest,
        channel: Option<ClientChannel>,
    ) -> ServiceResult<ItemQuoteView> {
        let (_, quote, methods) = self.prepare_item(client_id, item, channel).await?;
        Ok(ItemQuoteView { quote, methods })
    }

    pub async fn start_item(
        &self,
        client_id: ClientId,
        request: &ItemCheckoutRequest,
    ) -> ServiceResult<CheckoutResult> {
        let (client, quote, methods) = self
            .prepare_item(client_id, &request.item, request.channel.client())
            .await?;
        let (kind, amount, method) = pick_method(
            &methods,
            &request.provider,
            request.method.as_deref(),
            self.services.payments().balance_enabled(),
        )?;
        let order = NewOrder {
            client_id,
            subscription_id: request.item.subscription_id,
            kind: OrderKind::Module,
            intent: PurchaseIntent::Module {
                module: request.item.module,
                kind: request.item.kind.clone(),
                payload: request.item.payload.clone(),
            },
            quote: quote.clone(),
            snapshot: None,
        };
        let description = describe_for(
            &self.services.settings().brand_name,
            &quote,
            &request.provider,
        );
        self.settle(
            &client,
            Sale {
                order: &order,
                channel: request.channel.order(),
                provider: &request.provider,
                method,
                kind,
                amount,
                description,
                return_url: request.return_url.clone(),
            },
        )
        .await
    }

    async fn settle(&self, client: &Client, sale: Sale<'_>) -> ServiceResult<CheckoutResult> {
        let ctx = self.services.module_context();
        let hooks = OrderHooks {
            modules: self.services.modules(),
            ctx: &ctx,
        };
        if sale.amount.minor == 0 {
            let mut tx = self.services.store().begin().await?;
            let paid = record_paid(
                &mut tx,
                sale.order,
                sale.channel,
                providers::MANUAL,
                Some("free"),
                sale.amount,
                None,
                Some(&hooks),
            )
            .await?;
            tx.commit().await?;
            cancel_at_providers(self.services, &paid.replaced).await;
            return Ok(CheckoutResult::Completed {
                order_id: paid.order.public_id,
            });
        }
        match sale.kind {
            ProviderKind::Balance => {
                let mut tx = self.services.store().begin().await?;
                let paid = record_paid(
                    &mut tx,
                    sale.order,
                    sale.channel,
                    providers::BALANCE,
                    None,
                    sale.amount,
                    Some(LedgerKind::Purchase),
                    Some(&hooks),
                )
                .await?;
                tx.commit().await?;
                cancel_at_providers(self.services, &paid.replaced).await;
                Ok(CheckoutResult::Completed {
                    order_id: paid.order.public_id,
                })
            }
            ProviderKind::External | ProviderKind::Stars => {
                let provider = self
                    .services
                    .payments()
                    .get(sale.provider)
                    .ok_or(ServiceError::NotFound("платёжная система"))?;
                self.invoice(
                    client,
                    sale.order,
                    sale.channel,
                    PayWith {
                        provider,
                        method: sale.method,
                    },
                    sale.amount,
                    sale.description,
                    sale.return_url,
                )
                .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn invoice(
        &self,
        client: &Client,
        order: &NewOrder,
        channel: OrderChannel,
        pay_with: PayWith,
        amount: Amount,
        description: String,
        return_url: Option<String>,
    ) -> ServiceResult<CheckoutResult> {
        let PayWith { provider, method } = pay_with;
        let settings = self.services.settings();
        let public_url = settings
            .public_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());
        let external = provider.kind() == ProviderKind::External;
        if external && public_url.is_none() && !provider.polls_status() {
            return Err(ServiceError::rejected(
                "config.public_url",
                "Не задан внешний адрес панели: платёжная система не сможет прислать уведомление",
            ));
        }
        if amount.currency == PayCurrency::Xtr && provider.kind() != ProviderKind::Stars {
            return Err(ServiceError::rejected(
                "checkout.method_unavailable",
                "Звёздами оплачивается только счёт в Telegram",
            ));
        }
        let now = self.services.now();
        let ttl = Duration::minutes(settings.invoice_ttl_minutes as i64);
        let ctx = self.services.module_context();
        let modules = self.services.modules();
        let mut tx = self.services.store().begin().await?;
        let created = tx.orders().create(order).await?;
        tx.orders().set_channel(created.id, channel).await?;
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: created.id,
                client_id: created.client_id,
                provider: ProviderId::from(provider.id()),
                method: None,
                amount,
                expires_at: Some(now + ttl),
            })
            .await?;
        if let Some(code) = method.as_deref() {
            tx.payments().set_pay_method(payment.id, code).await?;
        }
        if created.kind == OrderKind::TopUp {
            let terms = amount.as_money().and_then(|money| {
                self.services.payments().topup_bonus(
                    provider.id(),
                    method.as_deref(),
                    money.currency,
                )
            });
            if let Some(terms) = terms {
                tx.payments().promise_bonus(payment.id, &terms).await?;
            }
        }
        let replaced = detach_unpaid(modules, &mut tx, &created).await?;
        modules
            .order_created(&ctx, &mut tx, &created, &payment)
            .await?;
        if external && (provider.accepts_http_webhooks() || provider.polls_status()) {
            let delay = if provider.polls_status() {
                CHAIN_POLL_DELAY_SECONDS
            } else {
                POLL_DELAY_SECONDS
            };
            tx.jobs()
                .enqueue(
                    &JobPayload::PollPayment {
                        payment_id: payment.id,
                    },
                    Some(now + Duration::seconds(delay)),
                )
                .await?;
        }
        let client_ref = client_ref(&mut tx, client).await?;
        tx.commit().await?;
        let plan = match (&order.intent, method.as_deref()) {
            (
                PurchaseIntent::Subscription {
                    tariff_id,
                    period_id,
                    ..
                },
                Some(sn_domain::payment::RECURRING_METHOD),
            ) => Some(sn_payments::PlanBinding {
                tariff_id: tariff_id.0,
                period_id: period_id.0,
            }),
            _ => None,
        };
        let request = InvoiceRequest {
            plan,
            payment_id: payment.id,
            payment_public_id: payment.public_id,
            order_public_id: created.public_id,
            amount,
            description,
            client: client_ref,
            return_url: return_url.filter(|u| trusted_return_url(u, public_url.as_deref())),
            webhook_url: public_url
                .as_deref()
                .map(|base| format!("{base}/api/webhooks/pay/{}", provider.id()))
                .unwrap_or_default(),
            expires_in: ttl,
            save_payment_method: provider.supports_recurring()
                && settings.autorenew.use_saved_card
                && matches!(
                    created.kind,
                    OrderKind::New | OrderKind::Renewal | OrderKind::Change
                ),
            method,
        };
        let config = self.services.payments().config(provider.id());
        match provider.create_invoice(&config, &request).await {
            Ok(invoice) => {
                let mut conn = self.services.store().conn().await?;
                conn.payments()
                    .attach_invoice(
                        payment.id,
                        &InvoiceUpdate {
                            pay_url: Some(invoice.pay_url.clone()),
                            provider_txid: invoice.external_id.clone(),
                            expires_at: invoice.expires_at,
                            payload: invoice.payload.clone(),
                        },
                    )
                    .await?;
                crate::renewals::note_invoice(
                    &mut conn,
                    provider.id(),
                    payment.id,
                    &created,
                    &invoice,
                )
                .await?;
                drop(conn);
                self.finish_replaced(&created, &replaced).await;
                let pay_url = match &public_url {
                    Some(base) => format!("{base}/api/pay/{}", payment.public_id),
                    None => invoice.pay_url,
                };
                Ok(CheckoutResult::Redirect {
                    order_id: created.public_id,
                    payment_id: payment.public_id,
                    pay_url,
                })
            }
            Err(err) => {
                tracing::warn!(payment = %payment.id, provider = provider.id(), error = %err, "счёт не выставлен");
                let reason = err.to_string();
                let mut tx = self.services.store().begin().await?;
                match tx
                    .payments()
                    .transition(payment.id, PaymentStatus::Failed, None, Some(&reason))
                    .await
                {
                    Ok(_) | Err(StoreError::Invalid(_)) => {}
                    Err(other) => return Err(other.into()),
                }
                match tx
                    .orders()
                    .transition(created.id, OrderStatus::Canceled)
                    .await
                {
                    Ok((canceled, true)) => {
                        modules.order_canceled(&ctx, &mut tx, &canceled).await?;
                    }
                    Ok(_) | Err(StoreError::Invalid(_)) => {}
                    Err(other) => return Err(other.into()),
                }
                promo::restore_held(&mut tx, &replaced).await?;
                tx.commit().await?;
                Err(err.into())
            }
        }
    }

    async fn finish_replaced(&self, created: &Order, replaced: &[OrderId]) {
        if replaced.is_empty() {
            return;
        }
        let ctx = self.services.module_context();
        let finished = async {
            let mut tx = self.services.store().begin().await?;
            let dropped =
                cancel_replaced(self.services.modules(), &ctx, &mut tx, created, replaced).await?;
            tx.commit().await?;
            Ok::<_, ServiceError>(dropped)
        }
        .await;
        match finished {
            Ok(dropped) => cancel_at_providers(self.services, &dropped).await,
            Err(err) => {
                tracing::warn!(order = %created.id, error = %err, "старый счёт не отменён после выставления нового");
            }
        }
    }
}
