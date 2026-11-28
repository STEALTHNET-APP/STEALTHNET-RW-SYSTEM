use crate::Actor;
use crate::autorenew::{AUTORENEW_METHOD, CHARGED_AFTER_OFF_TEMPLATE};
use crate::checkout::{category_full, single_subscription};
use crate::error::{ServiceError, ServiceResult};
use crate::inbox::{BLOCKED_REFUND_REASON, BlockedRefund};
use crate::services::Services;
use crate::subscriptions::{
    actions, disable_remna_user, drop_everywhere, enable_remna_user, with_subscription_lock,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::client::{ClientStatus, IdentityKind};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{CategoryId, ClientId, JobId, OrderId, RemnaUserId, SquadId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::module::ModuleId;
use sn_domain::order::{Order, OrderKind, OrderStatus, PurchaseIntent};
use sn_domain::payment::Payment;
use sn_domain::plan::{
    CurrentTariff, FulfillmentPlan, HeldDevices, NewUserProfile, PlanAction, PlanInput, PlanKind,
    classify, plan,
};
use sn_domain::subscription::{
    AutoRenewState, ExtraTraffic, NameSource, RemnaLink, Subscription, SubscriptionKind, TRIAL_TAG,
    remna_description, remna_name, remna_name_base, remna_spare_name, remna_username,
};
use sn_domain::tariff::TariffSnapshot;
use sn_modules::ModuleError;
use sn_remnawave::{
    CreateUserRequest, RemnaError, RemnaUser, RemnawaveApi, UpdateUserRequest, UserStatus,
};
use sn_store::{
    Db, LockSpace, NewSubscription, PlanStage, PurchaseApplied, ReviewJob, ReviewOrder, StoreError,
    Tx, audit_entity,
};
use sqlx::types::Json;
use std::collections::HashSet;
use std::time::Duration;

#[cfg(test)]
mod addon_tests;
#[cfg(test)]
pub(crate) mod naming_tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;

const MIN_RETRY: Duration = Duration::from_secs(15);
const REFUND_WAIT: Duration = Duration::from_secs(300);
const USERNAME_TAKEN: &str = "A019";
const FINISH_ATTEMPTS: usize = 3;
const RESERVE_ATTEMPTS: usize = 5;
const NAME_CANDIDATES: u32 = 20;
const USERNAME_KEY: &str = "subscriptions_remna_username_key";
pub const ORDER_ATTACHED: &str = "order.attached";
const REFUND_PENDING_REASON: &str =
    "по платежу заказа ждём решения платёжной системы о возврате: выдача отложена";

pub const MAX_FULFILL_NOTE: usize = 500;
pub const REVIEW_ORDERS_LIMIT: i64 = 50;
pub const REVIEW_ORDERS_MAX: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAction {
    Redeliver,
    Recalculate,
    MarkFulfilled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewItem {
    #[serde(flatten)]
    pub order: ReviewOrder,
    pub actions: Vec<ReviewAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Redelivery {
    pub order_id: OrderId,
    pub job_id: JobId,
    pub state: &'static str,
    pub recalculated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Resumed {
    pub queued: Vec<OrderId>,
    pub kept: Vec<OrderId>,
}

pub const BLOCKED_HOLD: &str = "клиент заблокирован: заказ";
pub const REFUND_UNCONFIRMED_REASON: &str = "касса не ответила на автоматический возврат, пока клиент был заблокирован: проверьте в кабинете кассы, вернулись ли деньги, затем отметьте возврат или нажмите «Повторить выдачу»";
pub const UNBLOCKED_CAUSE: &str = "клиента разблокировали";
const FULFILLED_TEMPLATE: &str = "order.fulfilled";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Recalculation {
    pub order_id: OrderId,
    pub job_id: JobId,
    pub state: &'static str,
    pub plan: PlanStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CategoryGuard {
    New,
    Change,
}

async fn with_category_lock<T, F>(
    services: &Services,
    client_id: ClientId,
    work: F,
) -> Result<T, Halt>
where
    F: Future<Output = Result<T, Halt>>,
{
    let mut guard = services.store().begin().await?;
    guard.lock(LockSpace::Category, client_id.0).await?;
    let result = work.await;
    guard.rollback().await?;
    result
}

pub fn review_actions(order: &ReviewOrder) -> Vec<ReviewAction> {
    let mut actions = Vec::new();
    if !order.refund_pending {
        if order.plan != PlanStage::Stale {
            actions.push(ReviewAction::Redeliver);
        }
        let running = order.job.as_ref().is_some_and(ReviewJob::active);
        let resettable = match order.plan {
            PlanStage::None => false,
            PlanStage::Stale => true,
            PlanStage::Saved | PlanStage::Applied => !running,
        };
        if order.kind.needs_remnawave() && resettable {
            actions.push(ReviewAction::Recalculate);
        }
    }
    actions.push(ReviewAction::MarkFulfilled);
    actions
}

pub fn abandoned_reason(attempts: u32, error: &str) -> String {
    format!(
        "выдача не завершилась за {attempts} попыток: {error}. План мог быть отправлен в Remnawave: \
         проверьте подписку и выберите «Пересчитать и выдать» или «Отметить выданным»"
    )
}

pub async fn holds_autorenew(db: &mut impl Db, order_id: OrderId) -> ServiceResult<bool> {
    Ok(db
        .orders()
        .holds_renewal(order_id, AUTORENEW_METHOD)
        .await?)
}

fn not_deliverable(status: OrderStatus) -> ServiceError {
    let message = match status {
        OrderStatus::Fulfilled => "Заказ уже выдан",
        OrderStatus::Refunded => "Заказ возвращён, выдавать его нельзя",
        OrderStatus::Pending | OrderStatus::Canceled => "Заказ не оплачен",
        OrderStatus::Paid | OrderStatus::Review => "Заказ нельзя выдать повторно",
    };
    ServiceError::rejected("order.redeliver_status", message)
}

fn not_in_review(status: OrderStatus) -> ServiceError {
    let message = match status {
        OrderStatus::Fulfilled => "Заказ уже выдан",
        OrderStatus::Refunded => "Заказ возвращён",
        OrderStatus::Pending | OrderStatus::Canceled => "Заказ не оплачен",
        OrderStatus::Paid => "Заказ не на ручной проверке: его выдаёт очередь",
        OrderStatus::Review => "Действие с заказом сейчас недоступно",
    };
    ServiceError::rejected("order.review_status", message)
}

fn awaiting_payment() -> ServiceError {
    ServiceError::rejected(
        "order.awaiting_payment",
        "Платёж по заказу ещё не получен: платёжная система задержала его для проверки. \
         Выдать заказ можно после оплаты",
    )
}

pub const AWAITING_PAYMENT_REASON: &str =
    "заказ ещё не оплачен: платёжная система задержала платёж для проверки";

fn refund_pending() -> ServiceError {
    ServiceError::rejected(
        "order.refund_pending",
        "По платежу этого заказа ждём решения платёжной системы о возврате. \
         Выдать заказ можно после отказа в возврате",
    )
}

fn panel_unavailable() -> ServiceError {
    ServiceError::rejected(
        "order.panel_unavailable",
        "Не удалось проверить подписку в Remnawave: попробуйте ещё раз позже",
    )
}

fn term_changed() -> ServiceError {
    ServiceError::rejected(
        "order.plan_stale",
        "План выдачи этого заказа рассчитан раньше и не применён, а срок подписки в Remnawave \
         с тех пор изменился. Выберите «Пересчитать и выдать» или «Отметить выданным»",
    )
}

fn plan_brief(plan: &FulfillmentPlan) -> serde_json::Value {
    serde_json::json!({
        "kind": plan.kind,
        "base_expire_at": plan.base_expire_at,
        "target_expire_at": plan.target.expire_at,
    })
}

fn delivered_kind(order: &Order, subscription: &Subscription) -> SubscriptionKind {
    if matches!(order.intent, PurchaseIntent::Trial) {
        SubscriptionKind::Trial
    } else if let Some(granted) = order.intent.granted_kind() {
        if order.intent.custom_grant() || subscription.kind == SubscriptionKind::Trial {
            granted
        } else {
            subscription.kind
        }
    } else if matches!(order.intent, PurchaseIntent::AdminGrant { .. }) {
        if subscription.kind == SubscriptionKind::Trial {
            SubscriptionKind::Manual
        } else {
            subscription.kind
        }
    } else {
        SubscriptionKind::Paid
    }
}

fn days_until(from: DateTime<Utc>, to: DateTime<Utc>) -> u32 {
    let days = ((to - from).num_seconds().max(0) + 86_399) / 86_400;
    u32::try_from(days).unwrap_or(u32::MAX).max(1)
}

enum Panel {
    Skipped,
    Missing,
    Found {
        subscription: Box<Subscription>,
        user: Box<RemnaUser>,
    },
}

impl Panel {
    fn audit(&self) -> serde_json::Value {
        match self {
            Panel::Skipped => serde_json::json!("skipped"),
            Panel::Missing => serde_json::json!("missing"),
            Panel::Found { user, .. } => serde_json::json!({
                "user_id": user.id,
                "expire_at": user.expire_at,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FulfillmentOutcome {
    Done {
        subscription_id: Option<SubscriptionId>,
    },
    Retry {
        after_seconds: u64,
        reason: String,
    },
    Review {
        reason: String,
    },
    Refunded {
        reason: String,
    },
}

impl FulfillmentOutcome {
    pub fn retry(after: Duration, reason: impl Into<String>) -> Self {
        FulfillmentOutcome::Retry {
            after_seconds: after.as_secs(),
            reason: reason.into(),
        }
    }
}

enum Halt {
    Retry(String),
    Review(String),
    Stale(String),
    Blocked(String),
    Failed(ServiceError),
}

impl From<ServiceError> for Halt {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::RemnaNotConfigured(reason) => {
                Halt::Retry(format!("Remnawave не подключена: {reason}"))
            }
            other => Halt::Failed(other),
        }
    }
}

impl From<StoreError> for Halt {
    fn from(err: StoreError) -> Self {
        Halt::Failed(err.into())
    }
}

impl From<sqlx::Error> for Halt {
    fn from(err: sqlx::Error) -> Self {
        Halt::Failed(err.into())
    }
}

fn same_moment(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    a.timestamp_millis() == b.timestamp_millis()
}

async fn client_blocked(tx: &mut Tx, client_id: ClientId) -> Result<bool, Halt> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM clients WHERE id = $1 FOR SHARE")
            .bind(client_id.0)
            .fetch_optional(tx.conn())
            .await?;
    Ok(status.as_deref() == Some(ClientStatus::Blocked.as_str()))
}

async fn autorenew_charge(tx: &mut Tx, order_id: OrderId) -> Result<Option<Payment>, Halt> {
    Ok(tx
        .payments()
        .for_order(order_id)
        .await?
        .into_iter()
        .find(|p| p.status.is_paid_like() && p.method.as_deref() == Some(AUTORENEW_METHOD)))
}

fn db_count(value: u32, what: &str) -> Result<i32, Halt> {
    i32::try_from(value).map_err(|_| Halt::Review(format!("{what}: слишком большое значение")))
}

const SUBSCRIPTION_KINDS: &str = "kind NOT IN ('topup', 'module')";

fn stale_reason(saved: Option<String>) -> String {
    let choice =
        "заказ ждёт решения администратора: «Пересчитать и выдать» или «Отметить выданным»";
    match saved.filter(|reason| !reason.trim().is_empty()) {
        Some(reason) => format!("{choice}. Причина: {reason}"),
        None => choice.to_string(),
    }
}

const UNBLOCKED_DELIVERY: &str = "клиента разблокировали: заказ выдаётся";

fn halted(halt: Halt) -> ServiceError {
    match halt {
        Halt::Failed(err) => err,
        Halt::Retry(reason)
        | Halt::Review(reason)
        | Halt::Stale(reason)
        | Halt::Blocked(reason) => ServiceError::Internal(reason),
    }
}

fn blocked_halt(order: &Order) -> Halt {
    Halt::Blocked(format!(
        "{BLOCKED_HOLD} {} не выдаётся, после разблокировки он выдастся сам",
        order.public_id
    ))
}

fn remna_halt(context: &str, err: RemnaError) -> Halt {
    let reason = format!("{context}: {err}");
    if err.is_retryable()
        || matches!(
            err,
            RemnaError::Unauthorized(_) | RemnaError::NotConfigured(_) | RemnaError::Redirected(_)
        )
    {
        Halt::Retry(reason)
    } else {
        Halt::Review(reason)
    }
}

fn traffic_bytes(value: u64) -> Result<i64, Halt> {
    i64::try_from(value).map_err(|_| Halt::Review("лимит трафика слишком большой".into()))
}

fn owned_by(user: &RemnaUser, profile: &NewUserProfile) -> bool {
    user.description.as_deref() == Some(profile.description.as_str())
}

enum Found {
    User(Box<RemnaUser>),
    Missing,
    Foreign,
}

fn foreign_name(username: &str) -> Halt {
    Halt::Review(format!(
        "имя {username} в Remnawave занято чужим пользователем"
    ))
}

fn name_reserved_elsewhere(err: &StoreError) -> bool {
    err.constraint() == Some(USERNAME_KEY)
}

pub(crate) async fn name_base(db: &mut impl Db, client_id: ClientId) -> Result<String, StoreError> {
    let identities = db.clients().identities(client_id).await?;
    let telegram = identities.iter().find(|i| i.kind == IdentityKind::Telegram);
    let telegram_username = telegram
        .and_then(|i| i.meta.get("username"))
        .and_then(|v| v.as_str());
    let email = identities
        .iter()
        .find(|i| i.kind == IdentityKind::Email)
        .map(|i| i.value.as_str());
    Ok(remna_name_base(&NameSource {
        telegram_username,
        email,
        telegram_id: telegram.and_then(|i| i.value.parse().ok()),
        client_id,
    }))
}

fn paid_and_active(subscription: &Subscription, now: DateTime<Utc>) -> bool {
    subscription.kind != SubscriptionKind::Trial
        && subscription.remna.is_some()
        && subscription.mirror.expire_at.is_some_and(|at| at > now)
}

fn custom_grant_halt() -> Halt {
    Halt::Review(
        "у клиента уже есть действующая подписка, а выдача модулем создаёт отдельную: решите вручную"
            .into(),
    )
}

fn name_candidates(base: &str, ordinal: u32) -> Vec<String> {
    (ordinal..ordinal.saturating_add(NAME_CANDIDATES))
        .map(|n| remna_name(base, n))
        .collect()
}

struct Context {
    order: Order,
    snapshot: TariffSnapshot,
    subscription: Subscription,
    profile: NewUserProfile,
    converts_trial: bool,
}

pub struct Fulfillment<'a> {
    services: &'a Services,
}

impl<'a> Fulfillment<'a> {
    pub fn new(services: &'a Services) -> Self {
        Fulfillment { services }
    }

    pub async fn run(&self, order_id: OrderId) -> ServiceResult<FulfillmentOutcome> {
        let (order, waits_refund, unpaid) = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(order_id).await?;
            let waits_refund = conn.orders().refund_pending(order_id).await?;
            let unpaid = order.status == OrderStatus::Review
                && conn.payments().awaiting_payment(order_id).await?;
            (order, waits_refund, unpaid)
        };
        if unpaid {
            return Ok(FulfillmentOutcome::Review {
                reason: AWAITING_PAYMENT_REASON.into(),
            });
        }
        match order.status {
            OrderStatus::Fulfilled => {
                return Ok(FulfillmentOutcome::Done {
                    subscription_id: order.subscription_id,
                });
            }
            OrderStatus::Paid | OrderStatus::Review => {}
            other => {
                return Ok(FulfillmentOutcome::Review {
                    reason: format!("заказ в статусе {}", other.as_str()),
                });
            }
        }
        if waits_refund {
            tracing::info!(order = %order.id, "выдача отложена: по платежу ждём решения о возврате");
            return Ok(FulfillmentOutcome::retry(
                REFUND_WAIT,
                REFUND_PENDING_REASON,
            ));
        }
        let result = match &order.intent {
            PurchaseIntent::TopUp { .. } => self.topup(&order).await,
            PurchaseIntent::Module { module, .. } => match self.ensure_not_blocked(&order).await {
                Ok(()) => self.module(&order, *module).await,
                Err(halt) => Err(halt),
            },
            PurchaseIntent::Subscription { .. }
            | PurchaseIntent::Trial
            | PurchaseIntent::AdminGrant { .. }
            | PurchaseIntent::ModuleGrant { .. } => self.subscription(order.clone()).await,
            PurchaseIntent::Imported { .. } => Err(Halt::Review(
                "заказ перенесён из старой системы и не выдаётся".into(),
            )),
        };
        match result {
            Ok(subscription_id) => Ok(FulfillmentOutcome::Done { subscription_id }),
            Err(Halt::Retry(reason)) => {
                tracing::info!(order = %order.id, %reason, "выдача отложена");
                Ok(FulfillmentOutcome::retry(MIN_RETRY, reason))
            }
            Err(Halt::Review(reason)) => self.hold(&order, reason).await,
            Err(Halt::Blocked(reason)) => self.blocked_order(&order, reason).await,
            Err(Halt::Stale(reason)) => {
                tracing::warn!(order = %order.id, %reason, "заказ ждёт решения администратора");
                Ok(FulfillmentOutcome::Review { reason })
            }
            Err(Halt::Failed(err)) => Err(err),
        }
    }

    async fn hold(&self, order: &Order, reason: String) -> ServiceResult<FulfillmentOutcome> {
        tracing::warn!(order = %order.id, %reason, "выдача ждёт ручной проверки");
        let mut conn = self.services.store().conn().await?;
        match conn.orders().mark_review(order.id, &reason).await {
            Ok(()) | Err(StoreError::Invalid(_)) => {}
            Err(err) => return Err(err.into()),
        }
        Ok(FulfillmentOutcome::Review { reason })
    }

    async fn hold_blocked(
        &self,
        order: &Order,
        reason: String,
    ) -> ServiceResult<FulfillmentOutcome> {
        let mut tx = self.services.store().begin().await?;
        let held = client_blocked(&mut tx, order.client_id)
            .await
            .map_err(halted)?;
        if held {
            match tx.orders().mark_review(order.id, &reason).await {
                Ok(()) | Err(StoreError::Invalid(_)) => {}
                Err(err) => return Err(err.into()),
            }
            tx.commit().await?;
        } else {
            drop(tx);
        }
        if held && self.blocked(order).await.map_err(halted)? {
            tracing::warn!(order = %order.id, %reason, "выдача ждёт разблокировки клиента");
            return Ok(FulfillmentOutcome::Review { reason });
        }
        tracing::info!(order = %order.id, "клиента разблокировали, пока заказ откладывался: заказ выдаётся");
        Ok(FulfillmentOutcome::retry(MIN_RETRY, UNBLOCKED_DELIVERY))
    }

    async fn blocked_order(
        &self,
        order: &Order,
        reason: String,
    ) -> ServiceResult<FulfillmentOutcome> {
        match self.services.inbox().refund_blocked(order.id).await? {
            BlockedRefund::Refunded => Ok(FulfillmentOutcome::Refunded {
                reason: BLOCKED_REFUND_REASON.into(),
            }),
            BlockedRefund::Requested => Ok(FulfillmentOutcome::retry(
                REFUND_WAIT,
                REFUND_PENDING_REASON,
            )),
            BlockedRefund::Retry(why) => {
                tracing::info!(order = %order.id, reason = %why, "выдача отложена");
                Ok(FulfillmentOutcome::retry(MIN_RETRY, why))
            }
            BlockedRefund::NothingPaid => self.hold_blocked(order, reason).await,
            BlockedRefund::Kept(why) => {
                self.hold(
                    order,
                    format!("{reason}. Оплату не вернули автоматически: {why}"),
                )
                .await
            }
            BlockedRefund::Unblocked => {
                tracing::info!(order = %order.id, "клиента разблокировали до автоматического возврата: заказ выдаётся");
                Ok(FulfillmentOutcome::retry(
                    MIN_RETRY,
                    "клиента разблокировали до автоматического возврата: заказ выдаётся",
                ))
            }
        }
    }

    pub async fn resume_blocked(&self, client_id: ClientId) -> ServiceResult<Resumed> {
        let waiting: Vec<i64> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_scalar(
                "SELECT o.id FROM orders o WHERE o.client_id = $1 AND o.status = 'review' \
                 AND starts_with(o.review_reason, $2) \
                 AND NOT EXISTS (SELECT 1 FROM payments p WHERE p.order_id = o.id AND ( \
                     p.refunded_minor > 0 OR p.status IN ('refunded', 'partially_refunded') \
                     OR EXISTS (SELECT 1 FROM refunds f WHERE f.payment_id = p.id) \
                     OR EXISTS (SELECT 1 FROM provider_refunds r WHERE r.payment_id = p.id \
                         AND r.status = 'pending'))) \
                 ORDER BY o.id",
            )
            .bind(client_id.0)
            .bind(BLOCKED_HOLD)
            .fetch_all(conn.conn())
            .await?
        };
        let mut resumed = Resumed::default();
        for order_id in waiting.into_iter().map(OrderId) {
            match self
                .redeliver_for(order_id, Actor::System, Some(UNBLOCKED_CAUSE))
                .await
            {
                Ok(_) => resumed.queued.push(order_id),
                Err(err) => {
                    tracing::warn!(order = %order_id, client = %client_id, error = %err, "заказ после разблокировки не поставлен на выдачу");
                    let mut conn = self.services.store().conn().await?;
                    let reason = format!("{UNBLOCKED_CAUSE}, но заказ не выдался сам. {err}");
                    match conn.orders().mark_review(order_id, &reason).await {
                        Ok(()) | Err(StoreError::Invalid(_)) => {}
                        Err(err) => return Err(err.into()),
                    }
                    resumed.kept.push(order_id);
                }
            }
        }
        Ok(resumed)
    }

    pub async fn review_orders(&self, limit: i64) -> ServiceResult<Vec<ReviewItem>> {
        let mut conn = self.services.store().conn().await?;
        let orders = conn
            .orders()
            .in_review(limit.clamp(1, REVIEW_ORDERS_MAX))
            .await?;
        Ok(orders
            .into_iter()
            .map(|order| ReviewItem {
                actions: review_actions(&order),
                order,
            })
            .collect())
    }

    pub async fn redeliver(&self, order_id: OrderId, actor: Actor) -> ServiceResult<Redelivery> {
        self.redeliver_for(order_id, actor, None).await
    }

    async fn redeliver_for(
        &self,
        order_id: OrderId,
        actor: Actor,
        cause: Option<&str>,
    ) -> ServiceResult<Redelivery> {
        let order = {
            let mut conn = self.services.store().conn().await?;
            conn.orders().get(order_id).await?
        };
        match order.subscription_id {
            Some(id) => {
                with_subscription_lock(self.services, id, self.requeue(order_id, actor, cause))
                    .await
            }
            None => self.requeue(order_id, actor, cause).await,
        }
    }

    async fn stale_term_kept(&self, order_id: OrderId) -> ServiceResult<bool> {
        let order = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(order_id).await?;
            if order.status != OrderStatus::Review
                || !conn.orders().plan_state(order_id).await?.stale()
            {
                return Ok(false);
            }
            order
        };
        let Some(plan) = order.plan.clone() else {
            return Ok(false);
        };
        let remna = self.services.remna().map_err(|_| panel_unavailable())?;
        match &plan.action {
            PlanAction::Update { user_id } => {
                let Some(base) = plan.base_expire_at else {
                    return Ok(false);
                };
                match remna.get_user(*user_id).await {
                    Ok(user) => Ok(same_moment(user.expire_at, base)),
                    Err(err) if err.is_not_found() => Ok(false),
                    Err(_) => Err(panel_unavailable()),
                }
            }
            PlanAction::Create { .. } => {
                let context = self.context(order).await.map_err(|_| panel_unavailable())?;
                match self.find_user(remna.as_ref(), &context).await {
                    Ok(Found::User(_)) => Ok(false),
                    Ok(Found::Missing | Found::Foreign) => Ok(true),
                    Err(_) => Err(panel_unavailable()),
                }
            }
        }
    }

    async fn requeue(
        &self,
        order_id: OrderId,
        actor: Actor,
        cause: Option<&str>,
    ) -> ServiceResult<Redelivery> {
        let kept = self.stale_term_kept(order_id).await?;
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order_id).await?;
        if !matches!(order.status, OrderStatus::Paid | OrderStatus::Review) {
            return Err(not_deliverable(order.status));
        }
        if tx.payments().awaiting_payment(order.id).await? {
            return Err(awaiting_payment());
        }
        if tx.orders().refund_pending(order.id).await? {
            return Err(refund_pending());
        }
        let stale =
            order.status == OrderStatus::Review && tx.orders().plan_state(order.id).await?.stale();
        if stale && !kept {
            return Err(term_changed());
        }
        let previous = if stale {
            tx.orders()
                .reset_plan(order.id)
                .await?
                .plan
                .as_ref()
                .map(plan_brief)
        } else {
            None
        };
        let queued = tx
            .jobs()
            .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
            .await?;
        tx.audit()
            .record(&actor.audit(
                "order.redeliver",
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "order_status": order.status,
                    "job_id": queued.id(),
                    "state": queued.state(),
                    "recalculated": stale,
                    "previous_plan": previous,
                    "reason": cause,
                }),
            ))
            .await?;
        tx.commit().await?;
        if stale {
            tracing::info!(order = %order.id, "срок подписки не менялся после расчёта: план пересчитан, заказ снова выдаётся");
        }
        Ok(Redelivery {
            order_id: order.id,
            job_id: queued.id(),
            state: queued.state(),
            recalculated: stale,
        })
    }

    pub async fn recalculate(
        &self,
        order_id: OrderId,
        actor: Actor,
    ) -> ServiceResult<Recalculation> {
        let order = {
            let mut conn = self.services.store().conn().await?;
            conn.orders().get(order_id).await?
        };
        match order.subscription_id {
            Some(id) => {
                with_subscription_lock(self.services, id, self.reset_plan(order_id, actor)).await
            }
            None => self.reset_plan(order_id, actor).await,
        }
    }

    async fn panel_user_missing(&self, order_id: OrderId) -> ServiceResult<bool> {
        let order = {
            let mut conn = self.services.store().conn().await?;
            if !conn.orders().plan_state(order_id).await?.applied() {
                return Ok(false);
            }
            conn.orders().get(order_id).await?
        };
        let Some(plan) = order.plan.clone() else {
            return Ok(false);
        };
        let remna = self.services.remna()?;
        match &plan.action {
            PlanAction::Update { user_id } => match remna.get_user(*user_id).await {
                Ok(_) => Ok(false),
                Err(err) if err.is_not_found() => Ok(true),
                Err(_) => Err(panel_unavailable()),
            },
            PlanAction::Create { .. } => {
                let context = self.context(order).await.map_err(|_| panel_unavailable())?;
                match self.current_user(remna.as_ref(), &context).await {
                    Ok(Some(_)) => Ok(false),
                    Ok(None) => Ok(true),
                    Err(_) => Err(panel_unavailable()),
                }
            }
        }
    }

    async fn reset_plan(&self, order_id: OrderId, actor: Actor) -> ServiceResult<Recalculation> {
        let missing = self.panel_user_missing(order_id).await?;
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order_id).await?;
        if order.status != OrderStatus::Review {
            return Err(not_in_review(order.status));
        }
        if tx.payments().awaiting_payment(order.id).await? {
            return Err(awaiting_payment());
        }
        if !order.kind.needs_remnawave() {
            return Err(ServiceError::rejected(
                "order.recalculate_kind",
                "У этого заказа нет плана выдачи: используйте «Повторить выдачу»",
            ));
        }
        if tx.orders().refund_pending(order.id).await? {
            return Err(refund_pending());
        }
        let state = tx.orders().plan_state(order.id).await?;
        let running = tx.jobs().active_fulfillment(order.id).await?.is_some();
        if running && matches!(state.stage(), PlanStage::Saved | PlanStage::Applied) {
            return Err(ServiceError::rejected(
                "order.delivery_running",
                "Выдача этого заказа уже идёт: дождитесь результата",
            ));
        }
        if state.applied() && !missing {
            return Err(ServiceError::rejected(
                "order.plan_applied",
                "План этого заказа уже применён в Remnawave, и пользователь там есть: пересчёт \
                 прибавил бы дни второй раз. Выберите «Повторить выдачу» или «Отметить выданным»",
            ));
        }
        let reset = tx.orders().reset_plan(order.id).await?;
        let queued = tx
            .jobs()
            .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
            .await?;
        let previous = reset.plan.as_ref().map(plan_brief);
        tx.audit()
            .record(&actor.audit(
                "order.recalculate",
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "job_id": queued.id(),
                    "state": queued.state(),
                    "plan": reset.state.stage(),
                    "panel_user_missing": missing,
                    "previous_plan": previous,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(Recalculation {
            order_id: order.id,
            job_id: queued.id(),
            state: queued.state(),
            plan: reset.state.stage(),
        })
    }

    pub async fn mark_fulfilled(
        &self,
        order_id: OrderId,
        note: &str,
        actor: Actor,
    ) -> ServiceResult<Order> {
        let note = note.trim();
        if note.is_empty() || note.chars().count() > MAX_FULFILL_NOTE {
            return Err(ServiceError::rejected(
                "order.mark_fulfilled_note",
                format!(
                    "Напишите пояснение, почему заказ считается выданным: до {MAX_FULFILL_NOTE} символов"
                ),
            ));
        }
        let order = {
            let mut conn = self.services.store().conn().await?;
            conn.orders().get(order_id).await?
        };
        match order.subscription_id {
            Some(id) => {
                with_subscription_lock(self.services, id, self.close_order(order_id, note, actor))
                    .await
            }
            None => self.close_order(order_id, note, actor).await,
        }
    }

    async fn panel_user(
        &self,
        remna: &dyn RemnawaveApi,
        order: &Order,
        subscription: &Subscription,
    ) -> ServiceResult<Option<RemnaUser>> {
        let (user_id, owned) = match (&subscription.remna, order.plan.as_ref().map(|p| &p.action)) {
            (Some(link), _) => (Some(link.user_id), None),
            (None, Some(PlanAction::Update { user_id })) => (Some(*user_id), None),
            (
                None,
                Some(PlanAction::Create {
                    username,
                    description,
                    ..
                }),
            ) => (None, Some((username, description))),
            (None, None) => (None, None),
        };
        if let Some(user_id) = user_id {
            return match remna.get_user(user_id).await {
                Ok(user) => Ok(Some(user)),
                Err(err) if err.is_not_found() => Ok(None),
                Err(_) => Err(panel_unavailable()),
            };
        }
        let Some((username, description)) = owned else {
            return Ok(None);
        };
        match remna.get_user_by_username(username).await {
            Ok(user) if user.description.as_deref() == Some(description.as_str()) => Ok(Some(user)),
            Ok(_) => Ok(None),
            Err(err) if err.is_not_found() => Ok(None),
            Err(_) => Err(panel_unavailable()),
        }
    }

    async fn panel_state(&self, order_id: OrderId) -> ServiceResult<Panel> {
        let (order, subscription) = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(order_id).await?;
            let subscription = match order.subscription_id {
                Some(id) if order.status == OrderStatus::Review && order.kind.needs_remnawave() => {
                    conn.subscriptions().get(id).await?
                }
                _ => return Ok(Panel::Skipped),
            };
            (order, subscription)
        };
        let remna = self.services.remna().map_err(|_| panel_unavailable())?;
        Ok(
            match self
                .panel_user(remna.as_ref(), &order, &subscription)
                .await?
            {
                Some(user) => Panel::Found {
                    subscription: Box::new(subscription),
                    user: Box::new(user),
                },
                None => Panel::Missing,
            },
        )
    }

    async fn sync_from_panel(
        &self,
        tx: &mut Tx,
        order: &Order,
        subscription: &Subscription,
        user: &RemnaUser,
    ) -> ServiceResult<()> {
        if subscription.remna.as_ref().map(|link| link.user_id) != Some(user.id) {
            let linked = tx
                .subscriptions()
                .link_remna(
                    subscription.id,
                    &RemnaLink {
                        user_id: user.id,
                        username: user.username.clone(),
                        short_uuid: Some(user.short_uuid.clone()),
                    },
                )
                .await;
            match linked {
                Ok(()) => {}
                Err(StoreError::Conflict { .. }) => {
                    return Err(ServiceError::rejected(
                        "order.panel_user_taken",
                        format!(
                            "Пользователь Remnawave {} уже привязан к другой подписке: проверьте вручную",
                            user.id
                        ),
                    ));
                }
                Err(err) => return Err(err.into()),
            }
        }
        let mirror = user.mirror(subscription.mirror.devices_count, self.services.now());
        tx.subscriptions()
            .apply_mirror(subscription.id, &mirror)
            .await?;
        let later: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM orders WHERE subscription_id = $1 AND id > $2 \
             AND status = 'fulfilled' AND {SUBSCRIPTION_KINDS})"
        ))
        .bind(subscription.id.0)
        .bind(order.id.0)
        .fetch_one(tx.conn())
        .await?;
        let Some(snapshot) = order.snapshot.as_ref().filter(|_| !later) else {
            return Ok(());
        };
        let trial = matches!(order.intent, PurchaseIntent::Trial);
        let custom = order.intent.custom_grant();
        let (period_started_at, period_days) = match &order.plan {
            Some(plan) => (plan.period_started_at, plan.period_days.max(1)),
            None => {
                let started = order.paid_at.unwrap_or_else(|| self.services.now());
                (started, days_until(started, user.expire_at))
            }
        };
        let extra_devices = match &order.intent {
            PurchaseIntent::Subscription { extra_devices, .. } => *extra_devices,
            _ => 0,
        };
        tx.subscriptions()
            .apply_purchase(
                subscription.id,
                &PurchaseApplied {
                    tariff_id: (!trial && !custom).then_some(snapshot.tariff_id),
                    kind: delivered_kind(order, subscription),
                    extra_devices,
                    period_started_at,
                    period_days,
                },
            )
            .await?;
        if let PurchaseIntent::Subscription { period_id, .. } = &order.intent {
            let current = tx.subscriptions().lock(subscription.id).await?;
            let enabled = if subscription.kind == SubscriptionKind::Trial {
                self.services.settings().autorenew.enable_for_new
            } else {
                current.autorenew.enabled
            };
            tx.subscriptions()
                .set_autorenew(
                    subscription.id,
                    AutoRenewState {
                        enabled,
                        period_id: Some(*period_id),
                    },
                )
                .await?;
        } else if custom && subscription.autorenew.enabled {
            tx.subscriptions()
                .set_autorenew(subscription.id, AutoRenewState::default())
                .await?;
        }
        Ok(())
    }

    async fn close_order(
        &self,
        order_id: OrderId,
        note: &str,
        actor: Actor,
    ) -> ServiceResult<Order> {
        let panel = self.panel_state(order_id).await?;
        let mut tx = self.services.store().begin().await?;
        let order = tx.orders().lock(order_id).await?;
        if order.status != OrderStatus::Review {
            return Err(not_in_review(order.status));
        }
        if tx.payments().awaiting_payment(order.id).await? {
            return Err(awaiting_payment());
        }
        let state = tx.orders().plan_state(order.id).await?;
        let reason = tx.orders().review_reason(order.id).await?;
        let (fulfilled, changed) = tx
            .orders()
            .transition(order.id, OrderStatus::Fulfilled)
            .await?;
        if changed {
            tx.events()
                .push(&DomainEvent::order_fulfilled(
                    &fulfilled,
                    fulfilled.subscription_id,
                ))
                .await?;
        }
        let mut notified = false;
        if let Panel::Found { subscription, user } = &panel {
            self.sync_from_panel(&mut tx, &order, subscription, user)
                .await?;
            if changed && !tx.clients().get(order.client_id).await?.is_blocked() {
                tx.jobs()
                    .enqueue(
                        &JobPayload::Notify {
                            client_id: order.client_id,
                            template: FULFILLED_TEMPLATE.into(),
                            params: serde_json::json!({
                                "order_id": fulfilled.public_id,
                                "order_kind": fulfilled.kind,
                                "subscription_id": subscription.id,
                                "tariff": order.snapshot.as_ref().map(|s| s.name.clone()),
                                "expire_at": user.expire_at,
                            }),
                        },
                        None,
                    )
                    .await?;
                notified = true;
            }
        }
        tx.audit()
            .record(&actor.audit(
                "order.mark_fulfilled",
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "note": note,
                    "review_reason": reason,
                    "plan": state.stage(),
                    "panel": panel.audit(),
                    "notified": notified,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(fulfilled)
    }

    async fn topup(&self, order: &Order) -> Result<Option<SubscriptionId>, Halt> {
        let mut tx = self.services.store().begin().await?;
        let paid = tx
            .payments()
            .for_order(order.id)
            .await?
            .into_iter()
            .find(|p| p.status.is_paid_like())
            .ok_or_else(|| Halt::Review("у пополнения нет оплаченного платежа".into()))?;
        let paid = tx.payments().lock(paid.id).await?;
        crate::inbox::credit_topup(&mut tx, order, &paid).await?;
        let (fulfilled, changed) = tx
            .orders()
            .transition(order.id, OrderStatus::Fulfilled)
            .await?;
        if changed {
            tx.events()
                .push(&DomainEvent::order_fulfilled(&fulfilled, None))
                .await?;
        }
        tx.commit().await?;
        Ok(None)
    }

    async fn module(
        &self,
        order: &Order,
        module: ModuleId,
    ) -> Result<Option<SubscriptionId>, Halt> {
        let registry = self.services.modules();
        if !registry.is_enabled(module) {
            return Err(Halt::Review(format!(
                "модуль «{}» выключен, покупку нужно выдать вручную",
                module.info().title
            )));
        }
        let ctx = self.services.module_context();
        registry
            .get(module)
            .fulfill(&ctx, order)
            .await
            .map_err(|err| match err {
                ModuleError::Store(_) | ModuleError::Core(_) => Halt::Retry(err.to_string()),
                other => Halt::Review(other.to_string()),
            })?;
        let mut tx = self.services.store().begin().await?;
        let (fulfilled, changed) = tx
            .orders()
            .transition(order.id, OrderStatus::Fulfilled)
            .await?;
        if changed {
            tx.events()
                .push(&DomainEvent::order_fulfilled(
                    &fulfilled,
                    fulfilled.subscription_id,
                ))
                .await?;
        }
        tx.commit().await?;
        Ok(order.subscription_id)
    }

    async fn blocked(&self, order: &Order) -> Result<bool, Halt> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.clients().get(order.client_id).await?.is_blocked())
    }

    async fn ensure_not_blocked(&self, order: &Order) -> Result<(), Halt> {
        if self.blocked(order).await? {
            return Err(blocked_halt(order));
        }
        Ok(())
    }

    async fn created_earlier(&self, remna: &dyn RemnawaveApi, order: &Order) -> Result<bool, Halt> {
        match &order.plan {
            Some(plan) if matches!(plan.action, PlanAction::Create { .. }) => {
                let context = self.context(order.clone()).await?;
                Ok(matches!(
                    self.find_user(remna, &context).await?,
                    Found::User(_)
                ))
            }
            _ => Ok(false),
        }
    }

    async fn subscription(&self, order: Order) -> Result<Option<SubscriptionId>, Halt> {
        let remna = self.services.remna()?;
        if order.subscription_id.is_none() {
            self.ensure_not_blocked(&order).await?;
        }
        let snapshot = order
            .snapshot
            .clone()
            .ok_or_else(|| Halt::Review("в заказе нет снимка тарифа".into()))?;
        match self.category_guard(&order, &snapshot).await? {
            Some(CategoryGuard::Change) => {
                with_category_lock(
                    self.services,
                    order.client_id,
                    Box::pin(self.change_category(&*remna, &order, &snapshot)),
                )
                .await
            }
            Some(CategoryGuard::New) => {
                with_category_lock(
                    self.services,
                    order.client_id,
                    Box::pin(self.deliver(&*remna, &order, &snapshot)),
                )
                .await
            }
            None => Box::pin(self.deliver(&*remna, &order, &snapshot)).await,
        }
    }

    async fn deliver(
        &self,
        remna: &dyn RemnawaveApi,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Option<SubscriptionId>, Halt> {
        let subscription = self.ensure_subscription(order, snapshot).await?;
        with_subscription_lock(
            self.services,
            subscription.id,
            Box::pin(self.locked(remna, order.id)),
        )
        .await
    }

    async fn category_guard(
        &self,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Option<CategoryGuard>, Halt> {
        if !self.services.settings().multi_subscriptions {
            return Ok(None);
        }
        let limited = match &order.intent {
            PurchaseIntent::Subscription { .. } => true,
            PurchaseIntent::ModuleGrant { tariff_id, .. } => tariff_id.is_some(),
            _ => false,
        };
        if !limited {
            return Ok(None);
        }
        let mut conn = self.services.store().conn().await?;
        let limit = match conn.tariffs().category(snapshot.category_id).await {
            Ok(category) => category.subscription_limit,
            Err(StoreError::NotFound(_)) => None,
            Err(err) => return Err(err.into()),
        };
        if limit.is_none() {
            return Ok(None);
        }
        let Some(target) = order.subscription_id else {
            return Ok(Some(CategoryGuard::New));
        };
        if conn.orders().plan_state(order.id).await?.applied() {
            return Ok(None);
        }
        let subscription = conn.subscriptions().get(target).await?;
        let category = match subscription.tariff_id {
            Some(id) => match conn.tariffs().get(id).await {
                Ok(tariff) => Some(tariff.category_id),
                Err(StoreError::NotFound(_)) => None,
                Err(err) => return Err(err.into()),
            },
            None => None,
        };
        if category == Some(snapshot.category_id) {
            return Ok(None);
        }
        Ok(Some(CategoryGuard::Change))
    }

    async fn change_category(
        &self,
        remna: &dyn RemnawaveApi,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Option<SubscriptionId>, Halt> {
        let target = order
            .subscription_id
            .ok_or_else(|| Halt::Review("у заказа нет подписки".into()))?;
        let full = {
            let mut conn = self.services.store().conn().await?;
            category_full(&mut conn, true, order.client_id, snapshot.category_id).await?
        };
        if let Some(full) = full
            && !full.holds(target)
        {
            return Err(Halt::Review(format!(
                "в категории «{}» у клиента уже {} из {} разрешённых подписок, а этот заказ перевёл бы в неё ещё одну: решите вручную",
                full.category.name,
                full.held.len(),
                full.category.subscription_limit.unwrap_or_default()
            )));
        }
        self.deliver(remna, order, snapshot).await
    }

    async fn locked(
        &self,
        remna: &dyn RemnawaveApi,
        order_id: OrderId,
    ) -> Result<Option<SubscriptionId>, Halt> {
        let (order, state) = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(order_id).await?;
            let state = conn.orders().plan_state(order_id).await?;
            (order, state)
        };
        match order.status {
            OrderStatus::Fulfilled => return Ok(order.subscription_id),
            OrderStatus::Paid | OrderStatus::Review => {}
            other => {
                return Err(Halt::Review(format!("заказ в статусе {}", other.as_str())));
            }
        }
        if order.status == OrderStatus::Review && state.stale() {
            let mut conn = self.services.store().conn().await?;
            let saved = conn.orders().review_reason(order_id).await?;
            return Err(Halt::Stale(stale_reason(saved)));
        }
        if !state.applied()
            && self.blocked(&order).await?
            && !self.created_earlier(remna, &order).await?
        {
            return Err(blocked_halt(&order));
        }
        let context = self.context(order).await?;
        let (plan, saved) = match context.order.plan.clone() {
            Some(saved) => (saved, true),
            None => (self.prepare(remna, &context).await?, false),
        };
        let user = if state.applied() {
            self.applied_user(remna, &context, &plan).await?
        } else {
            if saved {
                self.check_base(remna, &context, &plan).await?;
            }
            self.ensure_no_refund(context.order.id).await?;
            let user = self.apply(remna, &context, &plan).await?;
            self.mark_applied(context.order.id).await?;
            user
        };
        let reset_done = user
            .last_traffic_reset_at
            .is_some_and(|at| at >= plan.period_started_at);
        let user = if plan.reset_traffic && !reset_done {
            remna
                .reset_user_traffic(user.id)
                .await
                .map_err(|e| remna_halt("сброс трафика", e))?
        } else {
            user
        };
        let fresh = remna
            .get_user(user.id)
            .await
            .map_err(|e| remna_halt("чтение пользователя после выдачи", e))?;
        self.finish(remna, &context, &plan, fresh).await?;
        Ok(Some(context.subscription.id))
    }

    async fn ensure_no_refund(&self, order_id: OrderId) -> Result<(), Halt> {
        let mut conn = self.services.store().conn().await?;
        if conn.orders().refund_pending_settled(order_id).await? {
            return Err(Halt::Retry(REFUND_PENDING_REASON.into()));
        }
        if crate::inbox::blocked_refund::unconfirmed(&mut conn, order_id).await? {
            return Err(Halt::Review(REFUND_UNCONFIRMED_REASON.into()));
        }
        Ok(())
    }

    async fn mark_applied(&self, order_id: OrderId) -> Result<(), Halt> {
        let mut conn = self.services.store().conn().await?;
        conn.orders()
            .mark_plan_applied(order_id, self.services.now())
            .await?;
        Ok(())
    }

    async fn applied_user(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<RemnaUser, Halt> {
        match &plan.action {
            PlanAction::Update { user_id } => remna.get_user(*user_id).await.map_err(|err| {
                if err.is_not_found() {
                    Halt::Review(format!(
                        "пользователь Remnawave {user_id} удалён после выдачи: проверьте вручную"
                    ))
                } else {
                    remna_halt("чтение пользователя после выдачи", err)
                }
            }),
            PlanAction::Create { username, .. } => match self.current_user(remna, context).await? {
                Some(user) => Ok(user),
                None => Err(Halt::Review(format!(
                    "созданный пользователь {username} не найден в Remnawave: проверьте вручную"
                ))),
            },
        }
    }

    async fn check_base(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<(), Halt> {
        let (PlanAction::Update { user_id }, Some(base)) = (&plan.action, plan.base_expire_at)
        else {
            return Ok(());
        };
        let target = plan.target.expire_at;
        let expected = |at: DateTime<Utc>| same_moment(at, base) || same_moment(at, target);
        if context.subscription.mirror.expire_at.is_none_or(expected) {
            return Ok(());
        }
        let current = remna.get_user(*user_id).await.map_err(|err| {
            if err.is_not_found() {
                Halt::Review(format!(
                    "пользователь Remnawave {user_id} удалён до выдачи: выдайте вручную"
                ))
            } else {
                remna_halt("проверка срока перед выдачей", err)
            }
        })?;
        if expected(current.expire_at) {
            return Ok(());
        }
        Err(Halt::Review(format!(
            "срок подписки в Remnawave изменился после расчёта плана: был {base}, сейчас {}, \
             по плану {target}. Проверьте подписку: если срок ещё не учитывает этот заказ, выберите \
             «Пересчитать и выдать», если уже учитывает, выберите «Отметить выданным»",
            current.expire_at
        )))
    }

    async fn context(&self, order: Order) -> Result<Context, Halt> {
        let snapshot = order
            .snapshot
            .clone()
            .ok_or_else(|| Halt::Review("в заказе нет снимка тарифа".into()))?;
        let subscription = self.ensure_subscription(&order, &snapshot).await?;
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(order.client_id).await?;
        let identities = conn.clients().identities(order.client_id).await?;
        let username = match &subscription.remna {
            Some(link) => link.username.clone(),
            None => conn
                .subscriptions()
                .reserved_username(subscription.id)
                .await?
                .unwrap_or_else(|| remna_username(subscription.public_id)),
        };
        let profile = NewUserProfile {
            username,
            telegram_id: identities
                .iter()
                .find(|i| i.kind == IdentityKind::Telegram)
                .and_then(|i| i.value.parse().ok()),
            email: identities
                .iter()
                .find(|i| i.kind == IdentityKind::Email)
                .map(|i| i.value.clone()),
            description: remna_description(client.public_id),
        };
        let converts_trial = subscription.kind == SubscriptionKind::Trial
            && !matches!(order.intent, PurchaseIntent::Trial);
        Ok(Context {
            order,
            snapshot,
            subscription,
            profile,
            converts_trial,
        })
    }

    async fn ensure_subscription(
        &self,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Subscription, Halt> {
        if let Some(id) = order.subscription_id {
            let mut conn = self.services.store().conn().await?;
            return Ok(conn.subscriptions().get(id).await?);
        }
        for _ in 0..RESERVE_ATTEMPTS {
            if let Some(subscription) = self.create_subscription(order, snapshot).await? {
                return Ok(subscription);
            }
        }
        Err(Halt::Retry(
            "имя пользователя Remnawave заняли параллельно, повторим".into(),
        ))
    }

    async fn attach_single(
        &self,
        tx: &mut Tx,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Option<Subscription>, Halt> {
        let grant = matches!(order.intent, PurchaseIntent::AdminGrant { .. });
        let module_grant = matches!(order.intent, PurchaseIntent::ModuleGrant { .. });
        let purchase = matches!(
            order.intent,
            PurchaseIntent::Subscription { target: None, .. }
        );
        let (existing, category, reason) = if self.services.settings().multi_subscriptions {
            if !(purchase || (module_grant && !order.intent.custom_grant())) {
                return Ok(None);
            }
            let Some(full) = category_full(tx, true, order.client_id, snapshot.category_id).await?
            else {
                return Ok(None);
            };
            let Some(latest) = full.held.iter().rev().find(|s| s.remna.is_some()).cloned() else {
                return Err(Halt::Retry(format!(
                    "сначала выдаётся подписка категории «{}», заказ добавится к ней",
                    full.category.name
                )));
            };
            (
                latest,
                Some(snapshot.category_id),
                format!(
                    "подписок категории «{}» у клиента может быть не больше {}",
                    full.category.name,
                    full.category.subscription_limit.unwrap_or_default()
                ),
            )
        } else {
            if !(grant || module_grant || purchase) {
                return Ok(None);
            }
            let Some((existing, category)) =
                single_subscription(tx, order.client_id, snapshot.category_id).await?
            else {
                return Ok(None);
            };
            if order.intent.custom_grant() && paid_and_active(&existing, self.services.now()) {
                return Err(custom_grant_halt());
            }
            (
                existing,
                category,
                "у клиента может быть только одна подписка".to_string(),
            )
        };
        let kind = if grant {
            OrderKind::AdminGrant
        } else if module_grant {
            OrderKind::ModuleGrant
        } else {
            classify(
                snapshot.category_id,
                Some(CurrentTariff {
                    kind: existing.kind,
                    category_id: category,
                }),
            )
            .order_kind()
        };
        tx.orders().attach(order.id, existing.id, kind).await?;
        tx.audit()
            .record(&Actor::System.audit(
                ORDER_ATTACHED,
                audit_entity::ORDER,
                Some(order.id.0),
                serde_json::json!({
                    "subscription_id": existing.id,
                    "order_kind": kind,
                    "reason": reason,
                }),
            ))
            .await?;
        Ok(Some(existing))
    }

    async fn create_subscription(
        &self,
        order: &Order,
        snapshot: &TariffSnapshot,
    ) -> Result<Option<Subscription>, Halt> {
        let mut tx = self.services.store().begin().await?;
        let locked = tx.orders().lock(order.id).await?;
        if let Some(id) = locked.subscription_id {
            let subscription = tx.subscriptions().get(id).await?;
            tx.commit().await?;
            return Ok(Some(subscription));
        }
        if let Some(existing) = self.attach_single(&mut tx, &locked, snapshot).await? {
            tx.commit().await?;
            return Ok(Some(existing));
        }
        let settings = self.services.settings();
        let (kind, tariff_id, autorenew, extra_devices) = match &order.intent {
            PurchaseIntent::Trial => (SubscriptionKind::Trial, None, AutoRenewState::default(), 0),
            PurchaseIntent::AdminGrant { period_id, .. } => (
                SubscriptionKind::Manual,
                Some(snapshot.tariff_id),
                AutoRenewState {
                    enabled: false,
                    period_id: *period_id,
                },
                0,
            ),
            PurchaseIntent::Subscription {
                period_id,
                extra_devices,
                ..
            } => (
                SubscriptionKind::Paid,
                Some(snapshot.tariff_id),
                AutoRenewState {
                    enabled: settings.autorenew.enable_for_new,
                    period_id: Some(*period_id),
                },
                *extra_devices,
            ),
            PurchaseIntent::ModuleGrant {
                tariff_id,
                period_id,
                ..
            } => (
                order
                    .intent
                    .granted_kind()
                    .unwrap_or(SubscriptionKind::Manual),
                *tariff_id,
                AutoRenewState {
                    enabled: false,
                    period_id: *period_id,
                },
                0,
            ),
            PurchaseIntent::TopUp { .. }
            | PurchaseIntent::Module { .. }
            | PurchaseIntent::Imported { .. } => {
                return Err(Halt::Review("заказ не выдаёт подписку".into()));
            }
        };
        let created = tx
            .subscriptions()
            .create(&NewSubscription {
                client_id: order.client_id,
                tariff_id,
                kind,
                extra_devices,
                autorenew,
            })
            .await?;
        let base = name_base(&mut tx, order.client_id).await?;
        let ordinal = tx
            .subscriptions()
            .ordinal(order.client_id, Some(created.id))
            .await?;
        let candidates = name_candidates(&base, ordinal);
        let taken = tx.subscriptions().taken_usernames(&candidates).await?;
        let name = candidates
            .into_iter()
            .find(|name| !taken.contains(name))
            .unwrap_or_else(|| remna_spare_name(&base, created.public_id));
        match tx.subscriptions().reserve_username(created.id, &name).await {
            Ok(()) => {}
            Err(err) if name_reserved_elsewhere(&err) => {
                tx.rollback().await?;
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        }
        tx.orders().set_subscription(order.id, created.id).await?;
        tx.commit().await?;
        Ok(Some(created))
    }

    async fn free_name(
        &self,
        remna: &dyn RemnawaveApi,
        subscription: &Subscription,
    ) -> Result<String, Halt> {
        let mut conn = self.services.store().conn().await?;
        let base = name_base(&mut conn, subscription.client_id).await?;
        let ordinal = conn
            .subscriptions()
            .ordinal(subscription.client_id, Some(subscription.id))
            .await?;
        let candidates = name_candidates(&base, ordinal);
        let taken = conn.subscriptions().taken_usernames(&candidates).await?;
        drop(conn);
        for name in candidates.into_iter().filter(|name| !taken.contains(name)) {
            match remna.get_user_by_username(&name).await {
                Err(err) if err.is_not_found() => return Ok(name),
                Ok(_) => {}
                Err(err) => return Err(remna_halt("поиск свободного имени", err)),
            }
        }
        Ok(remna_spare_name(&base, subscription.public_id))
    }

    async fn rename(
        &self,
        remna: &dyn RemnawaveApi,
        subscription: &Subscription,
        from: &str,
    ) -> Result<String, Halt> {
        if subscription.remna.is_some() {
            return Err(foreign_name(from));
        }
        for _ in 0..RESERVE_ATTEMPTS {
            let name = self.free_name(remna, subscription).await?;
            let mut conn = self.services.store().conn().await?;
            match conn
                .subscriptions()
                .rename_reserved(subscription.id, from, &name)
                .await
            {
                Ok(true) => {
                    tracing::info!(
                        subscription = %subscription.id,
                        from,
                        to = %name,
                        "имя в Remnawave занято чужим пользователем, выбрано следующее свободное"
                    );
                    return Ok(name);
                }
                Ok(false) => {
                    return Err(Halt::Retry(
                        "резерв имени подписки изменился, повторим".into(),
                    ));
                }
                Err(err) if name_reserved_elsewhere(&err) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Err(foreign_name(from))
    }

    async fn wait_for_others(&self, context: &Context) -> Result<(), Halt> {
        let mut conn = self.services.store().conn().await?;
        match conn
            .orders()
            .delivery_blocker(context.subscription.id, context.order.id)
            .await?
        {
            Some(public_id) => Err(Halt::Retry(format!("сначала выдаётся заказ {public_id}"))),
            None => Ok(()),
        }
    }

    async fn previous_snapshot(&self, context: &Context) -> Result<Option<TariffSnapshot>, Halt> {
        let mut conn = self.services.store().conn().await?;
        let snapshot: Option<Json<TariffSnapshot>> = sqlx::query_scalar(&format!(
            "SELECT snapshot FROM orders WHERE subscription_id = $1 AND id <> $2 \
             AND fulfilled_at IS NOT NULL AND snapshot IS NOT NULL AND {SUBSCRIPTION_KINDS} \
             ORDER BY fulfilled_at DESC, id DESC LIMIT 1"
        ))
        .bind(context.subscription.id.0)
        .bind(context.order.id.0)
        .fetch_optional(conn.conn())
        .await?;
        Ok(snapshot.map(|s| s.0))
    }

    async fn kept_squads(&self, context: &Context, user: &RemnaUser) -> Result<Vec<SquadId>, Halt> {
        let from_tariffs = match self.previous_snapshot(context).await? {
            Some(previous) => previous.squads,
            None => {
                let mut conn = self.services.store().conn().await?;
                conn.tariffs().squads_in_use().await?
            }
        };
        Ok(user
            .squad_ids()
            .into_iter()
            .filter(|s| !from_tariffs.contains(s))
            .collect())
    }

    async fn held_devices(
        &self,
        context: &Context,
        user: &RemnaUser,
        paid_at: DateTime<Utc>,
    ) -> Result<Option<HeldDevices>, Halt> {
        let mut conn = self.services.store().conn().await?;
        let stored: Option<(i32, DateTime<Utc>, i32)> = sqlx::query_as(
            "SELECT held_limit, held_until, limit_after FROM subscription_device_holds \
             WHERE subscription_id = $1",
        )
        .bind(context.subscription.id.0)
        .fetch_optional(conn.conn())
        .await?;
        let paid_after_hold = match stored {
            Some((limit, until, _)) if until > paid_at => {
                return Ok(u32::try_from(limit)
                    .ok()
                    .map(|limit| HeldDevices { limit, until }));
            }
            Some((_, _, after)) => Some(i64::from(after)),
            None => None,
        };
        if user.status == UserStatus::Expired || user.expire_at <= paid_at {
            return Ok(None);
        }
        Ok(user
            .hwid_device_limit
            .map(|limit| paid_after_hold.map_or(limit, |after| limit.min(after)))
            .and_then(|limit| u32::try_from(limit).ok())
            .map(|limit| HeldDevices {
                limit,
                until: user.expire_at,
            }))
    }

    async fn find_user(&self, remna: &dyn RemnawaveApi, context: &Context) -> Result<Found, Halt> {
        if let Some(link) = &context.subscription.remna {
            match remna.get_user(link.user_id).await {
                Ok(user) => return Ok(Found::User(Box::new(user))),
                Err(err) if err.is_not_found() => {}
                Err(err) => return Err(remna_halt("чтение пользователя", err)),
            }
        }
        match remna.get_user_by_username(&context.profile.username).await {
            Ok(user) if owned_by(&user, &context.profile) => Ok(Found::User(Box::new(user))),
            Ok(_) => Ok(Found::Foreign),
            Err(err) if err.is_not_found() => Ok(Found::Missing),
            Err(err) => Err(remna_halt("поиск пользователя по имени", err)),
        }
    }

    async fn current_user(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
    ) -> Result<Option<RemnaUser>, Halt> {
        match self.find_user(remna, context).await? {
            Found::User(user) => Ok(Some(*user)),
            Found::Missing => Ok(None),
            Found::Foreign => Err(foreign_name(&context.profile.username)),
        }
    }

    async fn subscription_category(&self, context: &Context) -> Result<Option<CategoryId>, Halt> {
        let Some(tariff_id) = context.subscription.tariff_id else {
            return Ok(None);
        };
        let mut conn = self.services.store().conn().await?;
        match conn.tariffs().get(tariff_id).await {
            Ok(tariff) => Ok(Some(tariff.category_id)),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn plan_kind(
        &self,
        context: &Context,
        current: Option<&RemnaUser>,
    ) -> Result<PlanKind, Halt> {
        Ok(match context.order.kind {
            OrderKind::New | OrderKind::Trial => PlanKind::New,
            OrderKind::Renewal => PlanKind::Renewal,
            OrderKind::Change => PlanKind::Change,
            OrderKind::AdminGrant | OrderKind::ModuleGrant if current.is_none() => PlanKind::New,
            OrderKind::AdminGrant | OrderKind::ModuleGrant => classify(
                context.snapshot.category_id,
                Some(CurrentTariff {
                    kind: context.subscription.kind,
                    category_id: self.subscription_category(context).await?,
                }),
            ),
            OrderKind::TopUp | OrderKind::Module => {
                return Err(Halt::Review("заказ не выдаёт подписку".into()));
            }
        })
    }

    async fn prepare(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
    ) -> Result<FulfillmentPlan, Halt> {
        self.wait_for_others(context).await?;
        if context.order.intent.custom_grant()
            && paid_and_active(&context.subscription, self.services.now())
        {
            return Err(custom_grant_halt());
        }
        let mut profile = context.profile.clone();
        let current = match self.find_user(remna, context).await? {
            Found::User(user) => Some(*user),
            Found::Missing => None,
            Found::Foreign => {
                profile.username = self
                    .rename(remna, &context.subscription, &context.profile.username)
                    .await?;
                None
            }
        };
        let kind = self.plan_kind(context, current.as_ref()).await?;
        let paid_at = context.order.paid_at.unwrap_or_else(|| self.services.now());
        let (keep_squads, held_devices) = match &current {
            Some(user) if kind == PlanKind::Renewal => (
                self.kept_squads(context, user).await?,
                self.held_devices(context, user, paid_at).await?,
            ),
            _ => (Vec::new(), None),
        };
        let extra_devices = match &context.order.intent {
            PurchaseIntent::Subscription { extra_devices, .. } => *extra_devices,
            _ => 0,
        };
        let manual_devices = match (kind, context.subscription.device_limit) {
            (PlanKind::Renewal, Some(manual)) => {
                let bought =
                    i64::from(extra_devices) - i64::from(context.subscription.extra_devices);
                Some(u32::try_from((i64::from(manual) + bought).max(1)).unwrap_or(manual))
            }
            _ => None,
        };
        let state = current.as_ref().map(RemnaUser::state);
        let tag = matches!(context.order.intent, PurchaseIntent::Trial).then_some(TRIAL_TAG);
        let (extra_traffic, grace) = {
            let mut conn = self.services.store().conn().await?;
            let extra = conn
                .subscriptions()
                .extra_traffic(context.subscription.id)
                .await?
                .map_or(0, |extra| extra.bytes);
            let grace =
                crate::renewals::grace_for(self.services, &mut conn, context.order.id).await?;
            (extra, grace)
        };
        let computed = plan(&PlanInput {
            grace,
            extra_traffic,
            kind,
            snapshot: &context.snapshot,
            extra_devices,
            held_devices,
            paid_at,
            current: state.as_ref(),
            keep_squads: &keep_squads,
            profile: &profile,
            tag,
            manual_devices,
        })
        .map_err(|e| Halt::Review(format!("план выдачи не построен: {e}")))?;
        let squads = remna
            .internal_squads()
            .await
            .map_err(|e| remna_halt("список сквадов", e))?;
        let known: HashSet<uuid::Uuid> = squads.internal_squads.iter().map(|s| s.uuid).collect();
        if let Some(missing) = computed
            .target
            .squads
            .iter()
            .find(|s| !known.contains(&s.0))
        {
            return Err(Halt::Review(format!(
                "сквада {missing} нет в Remnawave: проверьте сквады тарифа «{}»",
                context.snapshot.name
            )));
        }
        let mut conn = self.services.store().conn().await?;
        Ok(conn.orders().save_plan(context.order.id, &computed).await?)
    }

    async fn apply(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<RemnaUser, Halt> {
        let target = &plan.target;
        match &plan.action {
            PlanAction::Create {
                username,
                telegram_id,
                email,
                description,
            } => {
                let request = CreateUserRequest {
                    username: username.clone(),
                    status: None,
                    expire_at: target.expire_at,
                    traffic_limit_bytes: traffic_bytes(target.traffic_limit_bytes)?,
                    traffic_limit_strategy: target.traffic_strategy,
                    description: Some(description.clone()),
                    tag: target.tag.clone(),
                    telegram_id: *telegram_id,
                    email: email.clone(),
                    hwid_device_limit: target.hwid_device_limit.map(i64::from),
                    active_internal_squads: target.squads.iter().map(|s| s.0).collect(),
                    external_squad_uuid: target.external_squad.map(|s| s.0),
                    ..Default::default()
                };
                match remna.create_user(&request).await {
                    Ok(user) => Ok(user),
                    Err(err) if err.code() == Some(USERNAME_TAKEN) => {
                        let existing = remna
                            .get_user_by_username(username)
                            .await
                            .map_err(|e| remna_halt("поиск занятого имени", e))?;
                        if existing.description.as_deref() != Some(description.as_str()) {
                            return Err(self.taken_meanwhile(remna, context, username).await);
                        }
                        self.patch(remna, existing.id, context, plan).await
                    }
                    Err(err) => Err(remna_halt("создание пользователя", err)),
                }
            }
            PlanAction::Update { user_id } => self.patch(remna, *user_id, context, plan).await,
        }
    }

    async fn taken_meanwhile(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        username: &str,
    ) -> Halt {
        let renamed = match self.rename(remna, &context.subscription, username).await {
            Ok(renamed) => renamed,
            Err(halt) => return halt,
        };
        let mut conn = match self.services.store().conn().await {
            Ok(conn) => conn,
            Err(err) => return Halt::Failed(err.into()),
        };
        match conn.orders().reset_plan(context.order.id).await {
            Ok(_) => Halt::Retry(format!(
                "имя {username} в Remnawave заняли во время выдачи, выдаём под именем {renamed}"
            )),
            Err(err) => Halt::Failed(err.into()),
        }
    }

    async fn patch(
        &self,
        remna: &dyn RemnawaveApi,
        user_id: RemnaUserId,
        context: &Context,
        plan: &FulfillmentPlan,
    ) -> Result<RemnaUser, Halt> {
        let target = &plan.target;
        let mut request = UpdateUserRequest::new(user_id);
        request.expire_at = Some(target.expire_at);
        request.traffic_limit_bytes = Some(traffic_bytes(target.traffic_limit_bytes)?);
        request.traffic_limit_strategy = Some(target.traffic_strategy);
        request.hwid_device_limit = Some(target.hwid_device_limit.map(i64::from));
        request.active_internal_squads = Some(target.squads.iter().map(|s| s.0).collect());
        if target.external_squad.is_some() || plan.kind != PlanKind::Renewal {
            request.external_squad_uuid = Some(target.external_squad.map(|s| s.0));
        }
        request.tag = match &target.tag {
            Some(tag) => Some(Some(tag.clone())),
            None if context.converts_trial => Some(None),
            None => None,
        };
        if plan.activate {
            request.status = Some(UserStatus::Active);
        }
        remna.update_user(&request).await.map_err(|err| {
            if err.is_not_found() {
                Halt::Review(format!(
                    "пользователь Remnawave {user_id} удалён до выдачи: выдайте вручную"
                ))
            } else {
                remna_halt("обновление пользователя", err)
            }
        })
    }

    async fn cut_access(
        &self,
        remna: &dyn RemnawaveApi,
        user: &RemnaUser,
    ) -> Result<RemnaUser, Halt> {
        let disabled = disable_remna_user(remna, user.id)
            .await
            .map_err(|e| remna_halt("отключение подписки заблокированного клиента", e))?;
        if let Err(err) = drop_everywhere(remna, vec![user.id]).await {
            tracing::warn!(user = %user.id, error = %err, "подключения заблокированного клиента не сброшены");
        }
        Ok(disabled)
    }

    async fn finish(
        &self,
        remna: &dyn RemnawaveApi,
        context: &Context,
        plan: &FulfillmentPlan,
        user: RemnaUser,
    ) -> Result<(), Halt> {
        let mut user = user;
        let mut cut = false;
        for _ in 0..FINISH_ATTEMPTS {
            let mut tx = self.services.store().begin().await?;
            let blocked = client_blocked(&mut tx, context.order.client_id).await?;
            if blocked && user.status != UserStatus::Disabled {
                tx.rollback().await?;
                user = self.cut_access(remna, &user).await?;
                cut = true;
                continue;
            }
            if !blocked && cut && user.status == UserStatus::Disabled {
                tx.rollback().await?;
                user = enable_remna_user(remna, user.id)
                    .await
                    .map_err(|e| remna_halt("включение подписки после разблокировки", e))?;
                cut = false;
                continue;
            }
            return self.record(tx, context, plan, &user, blocked).await;
        }
        Err(Halt::Retry(
            "статус клиента меняется во время выдачи".into(),
        ))
    }

    async fn record(
        &self,
        mut tx: Tx,
        context: &Context,
        plan: &FulfillmentPlan,
        user: &RemnaUser,
        blocked: bool,
    ) -> Result<(), Halt> {
        let order = &context.order;
        let subscription = &context.subscription;
        let snapshot = &context.snapshot;
        let settings = self.services.settings();
        let trial = matches!(order.intent, PurchaseIntent::Trial);
        match tx
            .subscriptions()
            .link_remna(
                subscription.id,
                &RemnaLink {
                    user_id: user.id,
                    username: user.username.clone(),
                    short_uuid: Some(user.short_uuid.clone()),
                },
            )
            .await
        {
            Ok(()) => {}
            Err(StoreError::Conflict { message, .. }) => {
                return Err(Halt::Review(format!(
                    "пользователь Remnawave {} уже привязан к другой подписке: {message}",
                    user.id
                )));
            }
            Err(err) => return Err(err.into()),
        }
        let mirror = user.mirror(subscription.mirror.devices_count, self.services.now());
        tx.subscriptions()
            .apply_mirror(subscription.id, &mirror)
            .await?;
        let custom = order.intent.custom_grant();
        let kind = delivered_kind(order, subscription);
        let extra_devices = match &order.intent {
            PurchaseIntent::Subscription { extra_devices, .. } => *extra_devices,
            _ => 0,
        };
        tx.subscriptions()
            .apply_purchase(
                subscription.id,
                &PurchaseApplied {
                    tariff_id: (!trial && !custom).then_some(snapshot.tariff_id),
                    kind,
                    extra_devices,
                    period_started_at: plan.period_started_at,
                    period_days: plan.period_days.max(1),
                },
            )
            .await?;
        let manual = match (plan.kind, subscription.device_limit) {
            (PlanKind::Renewal, Some(_)) => plan
                .device_hold
                .map(|hold| hold.limit_after)
                .or(plan.target.hwid_device_limit),
            _ => None,
        };
        tx.subscriptions()
            .set_device_limit(subscription.id, manual)
            .await?;
        match plan.device_hold {
            Some(hold) => {
                sqlx::query(
                    "INSERT INTO subscription_device_holds \
                     (subscription_id, order_id, held_limit, held_until, limit_after) \
                     VALUES ($1, $2, $3, $4, $5) ON CONFLICT (subscription_id) DO UPDATE SET \
                     order_id = EXCLUDED.order_id, held_limit = EXCLUDED.held_limit, \
                     held_until = EXCLUDED.held_until, limit_after = EXCLUDED.limit_after",
                )
                .bind(subscription.id.0)
                .bind(order.id.0)
                .bind(db_count(hold.held_limit, "лимит устройств")?)
                .bind(hold.until)
                .bind(db_count(hold.limit_after, "лимит устройств")?)
                .execute(tx.conn())
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM subscription_device_holds WHERE subscription_id = $1")
                    .bind(subscription.id.0)
                    .execute(tx.conn())
                    .await?;
            }
        }
        if !plan.keep_traffic_window {
            tx.subscriptions()
                .set_extra_traffic(&ExtraTraffic {
                    subscription_id: subscription.id,
                    bytes: plan.extra_traffic_bytes,
                    window_started_at: None,
                    window_lifetime: None,
                    window_ends_at: None,
                })
                .await?;
        }
        if custom && subscription.autorenew.enabled {
            tx.subscriptions()
                .set_autorenew(subscription.id, AutoRenewState::default())
                .await?;
        }
        let charged_after_off = match &order.intent {
            PurchaseIntent::Subscription { period_id, .. } => {
                let current = tx.subscriptions().lock(subscription.id).await?;
                let enabled = if subscription.kind == SubscriptionKind::Trial {
                    settings.autorenew.enable_for_new
                } else {
                    current.autorenew.enabled
                };
                tx.subscriptions()
                    .set_autorenew(
                        subscription.id,
                        AutoRenewState {
                            enabled,
                            period_id: Some(*period_id),
                        },
                    )
                    .await?;
                if enabled {
                    None
                } else {
                    autorenew_charge(&mut tx, order.id).await?
                }
            }
            _ => None,
        };
        let (fulfilled, changed) = tx
            .orders()
            .transition(order.id, OrderStatus::Fulfilled)
            .await?;
        if changed {
            crate::renewals::after_fulfilled(&mut tx, &fulfilled, self.services.now()).await?;
            let event = match (order.kind, subscription.remna.is_some()) {
                (OrderKind::New | OrderKind::Trial, _)
                | (OrderKind::AdminGrant | OrderKind::ModuleGrant, false) => {
                    DomainEvent::SubscriptionCreated {
                        subscription_id: subscription.id,
                        client_id: order.client_id,
                        kind,
                    }
                }
                (OrderKind::Change, _) => DomainEvent::TariffChanged {
                    subscription_id: subscription.id,
                    client_id: order.client_id,
                    from: subscription.tariff_id,
                    to: snapshot.tariff_id,
                },
                _ => DomainEvent::SubscriptionRenewed {
                    subscription_id: subscription.id,
                    client_id: order.client_id,
                    expire_at: user.expire_at,
                },
            };
            tx.events().push(&event).await?;
            if let (Some(from), Some(to)) = (subscription.mirror.status, mirror.status)
                && from != to
            {
                tx.events()
                    .push(&DomainEvent::SubscriptionStatusChanged {
                        subscription_id: subscription.id,
                        client_id: order.client_id,
                        from: Some(from),
                        to,
                    })
                    .await?;
            }
            tx.events()
                .push(&DomainEvent::order_fulfilled(
                    &fulfilled,
                    Some(subscription.id),
                ))
                .await?;
            if blocked {
                tx.audit()
                    .record(&Actor::System.audit(
                        actions::BLOCKED_DELIVERY,
                        audit_entity::SUBSCRIPTION,
                        Some(subscription.id.0),
                        serde_json::json!({
                            "order_id": fulfilled.id,
                            "client_id": order.client_id,
                            "remna_user_id": user.id,
                            "status": mirror.status,
                            "reason": "клиент заблокирован во время выдачи заказа",
                        }),
                    ))
                    .await?;
            } else {
                tx.jobs()
                    .enqueue(
                        &JobPayload::Notify {
                            client_id: order.client_id,
                            template: FULFILLED_TEMPLATE.into(),
                            params: serde_json::json!({
                                "order_id": fulfilled.public_id,
                                "order_kind": fulfilled.kind,
                                "subscription_id": subscription.id,
                                "tariff": snapshot.name,
                                "expire_at": user.expire_at,
                            }),
                        },
                        None,
                    )
                    .await?;
                if let Some(charge) = charged_after_off {
                    tracing::info!(order = %fulfilled.id, subscription = %subscription.id, "списание ушло до выключения автопродления: подписка продлена, клиент предупреждён");
                    tx.jobs()
                        .enqueue(
                            &JobPayload::Notify {
                                client_id: order.client_id,
                                template: CHARGED_AFTER_OFF_TEMPLATE.into(),
                                params: serde_json::json!({
                                    "order_id": fulfilled.public_id,
                                    "subscription_id": subscription.id,
                                    "payment_id": charge.id,
                                    "amount": charge.amount.as_money().map(format::money),
                                    "expire_at": user.expire_at,
                                }),
                            },
                            None,
                        )
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
