use crate::Actor;
use crate::checkout::{client_ref, describe, record_paid_order};
use crate::error::{ServiceError, ServiceResult};
use crate::inbox::saved_method_context;
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::client::Client;
use sn_domain::format;
use sn_domain::ids::{ClientId, OrderId, PaymentId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, Wallet};
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_domain::order::{NewOrder, Order, OrderChannel, OrderStatus, PurchaseIntent};
use sn_domain::paging::PageRequest;
use sn_domain::payment::{NewPayment, Payment, PaymentStatus, ProviderId, providers};
use sn_domain::plan::{CurrentTariff, classify};
use sn_domain::pricing::{QuoteRequest, quote};
use sn_domain::subscription::{AutoRenewState, Subscription, SubscriptionStatus};
use sn_domain::tariff::{Tariff, TariffPeriod};
use sn_modules::QuoteHook;
use sn_payments::{
    ClientRef, InvoiceRequest, PaymentError, PaymentProvider, PaymentRef, ProviderConfig,
    ProviderKind, ProviderStatus, WebhookOutcome, WebhookReply,
};
use sn_store::{AttemptResult, Db, DueCursor, InvoiceUpdate, LockSpace, Tx, audit_entity};
use std::sync::Arc;

#[cfg(test)]
pub(crate) mod tests;

pub const AUTORENEW_METHOD: &str = sn_domain::payment::AUTORENEW_METHOD;
pub const CHARGE_UNCLEAR_AUDIT: &str = "payment.autorenew_unclear";
pub const CHARGE_BLOCKED_AUDIT: &str = "payment.autorenew_blocked";
pub const CHARGED_AFTER_OFF_TEMPLATE: &str = "autorenew.charged_after_off";
pub const ENDED_AFTER_DAYS: i64 = 90;
const DUE_BATCH: i64 = 500;
const DUE_MARGIN_DAYS: i64 = 7;
const END_BATCH: i64 = 500;
const END_RUN_LIMIT: usize = 5_000;
const POLL_DELAY_SECONDS: i64 = 120;
const HOLD_AFTER_RETRY_DAYS: i64 = 1;
const REPEAT_WINDOW_HOURS: i64 = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AutoRenewReport {
    pub due: usize,
    pub charged: usize,
    pub reminded: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EndReport {
    pub ended: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChargeCheck {
    Settled,
    Pending,
    Unclear(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    Charged,
    Reminded,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChargeState {
    Pending,
    Paid,
    Declined,
}

enum Repeat {
    Answered(Box<WebhookOutcome>),
    Pending,
    Unclear(String),
}

struct Slot<'s> {
    subscription: &'s Subscription,
    cycle: String,
    number: u32,
}

struct Card {
    provider: Arc<dyn PaymentProvider>,
    token: String,
}

fn renewal_period<'t>(
    tariff: &'t Tariff,
    subscription: &Subscription,
    currency: Currency,
) -> Option<&'t TariffPeriod> {
    let offered = tariff.offered_periods(currency);
    if let Some(id) = subscription.autorenew.period_id
        && let Some(period) = offered.iter().find(|p| p.id == id)
    {
        return Some(*period);
    }
    let days = i64::from(subscription.period_days.unwrap_or(30));
    offered
        .into_iter()
        .min_by_key(|p| (i64::from(p.days) - days).abs())
}

pub(crate) fn transient(err: &PaymentError) -> bool {
    err.is_retryable() || matches!(err, PaymentError::Http { status: 408 | 429 })
}

fn cycle_key(expire_at: DateTime<Utc>) -> String {
    expire_at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn charge_details(payment: &Payment, order: &Order, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "client_id": payment.client_id,
        "order_id": order.id,
        "subscription_id": order.subscription_id,
        "amount": payment.amount,
        "provider": payment.provider,
        "provider_txid": payment.provider_txid,
        "reason": reason,
    })
}

async fn last_audit_reason(
    db: &mut impl Db,
    payment_id: PaymentId,
    action: &str,
) -> ServiceResult<Option<String>> {
    let reason: Option<Option<String>> = sqlx::query_scalar(
        "SELECT details->>'reason' FROM audit_log \
         WHERE entity_kind = $1 AND entity_id = $2 AND action = $3 ORDER BY id DESC LIMIT 1",
    )
    .bind(audit_entity::PAYMENT)
    .bind(payment_id.0)
    .bind(action)
    .fetch_optional(db.conn())
    .await?;
    Ok(reason.flatten())
}

async fn renewal_obstacle(
    db: &mut impl Db,
    order: &Order,
    client: &Client,
) -> ServiceResult<Option<&'static str>> {
    if client.is_blocked() {
        return Ok(Some("клиент уже заблокирован"));
    }
    let Some(subscription_id) = order.subscription_id else {
        return Ok(Some("у заказа нет подписки"));
    };
    let subscription = db.subscriptions().get(subscription_id).await?;
    if subscription.ended_at.is_some() {
        return Ok(Some("подписка уже завершена"));
    }
    if !subscription.autorenew.enabled {
        return Ok(Some("автопродление уже выключено"));
    }
    if subscription.mirror.status == Some(SubscriptionStatus::Disabled) {
        return Ok(Some("подписка заблокирована"));
    }
    let cycle: Option<String> = sqlx::query_scalar(
        "SELECT period_key FROM autorenew_attempts \
         WHERE subscription_id = $1 AND order_id = $2 ORDER BY attempt_no DESC LIMIT 1",
    )
    .bind(subscription.id.0)
    .bind(order.id.0)
    .fetch_optional(db.conn())
    .await?;
    let Some(cycle) = cycle else {
        return Ok(Some("попытка автопродления по этому заказу не найдена"));
    };
    if subscription.mirror.expire_at.map(cycle_key).as_deref() != Some(cycle.as_str()) {
        return Ok(Some("срок подписки уже изменился"));
    }
    let undelivered: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM orders WHERE subscription_id = $1 AND id <> $2 \
         AND status IN ('paid', 'review') AND kind NOT IN ('topup', 'module') ORDER BY id",
    )
    .bind(subscription.id.0)
    .bind(order.id.0)
    .fetch_all(db.conn())
    .await?;
    for other in undelivered {
        if crate::fulfillment::holds_autorenew(db, OrderId(other)).await? {
            return Ok(Some("подписка уже оплачена другим заказом"));
        }
    }
    Ok(None)
}

pub struct AutoRenew<'a> {
    services: &'a Services,
}

impl<'a> AutoRenew<'a> {
    pub fn new(services: &'a Services) -> Self {
        AutoRenew { services }
    }

    fn due_window(&self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let settings = self.services.settings();
        let until = now + Duration::days(i64::from(settings.autorenew.charge_days_before));
        let since =
            now - Duration::days(i64::from(settings.autorenew.retry_days) + DUE_MARGIN_DAYS);
        (until, since)
    }

    pub async fn due(&self, now: DateTime<Utc>, limit: i64) -> ServiceResult<Vec<Subscription>> {
        self.due_after(now, None, limit).await
    }

    pub async fn due_after(
        &self,
        now: DateTime<Utc>,
        after: Option<DueCursor>,
        limit: i64,
    ) -> ServiceResult<Vec<Subscription>> {
        let (until, since) = self.due_window(now);
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .subscriptions()
            .autorenew_due(until, since, after, limit.max(0))
            .await?)
    }

    pub async fn run_due(&self, now: DateTime<Utc>) -> ServiceResult<AutoRenewReport> {
        self.run_due_in_pages(now, DUE_BATCH).await
    }

    async fn run_due_in_pages(
        &self,
        now: DateTime<Utc>,
        page: i64,
    ) -> ServiceResult<AutoRenewReport> {
        let mut report = AutoRenewReport::default();
        if self.services.remna().is_err() {
            return Ok(report);
        }
        let page = page.max(1);
        let mut cursor = None;
        loop {
            let due = self.due_after(now, cursor, page).await?;
            report.due += due.len();
            for subscription in &due {
                match self.renew(subscription, now).await {
                    Ok(Attempt::Charged) => report.charged += 1,
                    Ok(Attempt::Reminded) => report.reminded += 1,
                    Ok(Attempt::Skipped) => {}
                    Ok(Attempt::Failed) => report.failed += 1,
                    Err(err) => {
                        report.failed += 1;
                        tracing::warn!(subscription = %subscription.id, error = %err, "автопродление не выполнено");
                    }
                }
            }
            match due.last().and_then(DueCursor::of) {
                Some(next) if i64::try_from(due.len()).unwrap_or(i64::MAX) >= page => {
                    cursor = Some(next);
                }
                _ => break,
            }
        }
        Ok(report)
    }

    pub async fn end_expired(&self, now: DateTime<Utc>) -> ServiceResult<EndReport> {
        let retry_days = i64::from(self.services.settings().autorenew.retry_days);
        let keep = ENDED_AFTER_DAYS.max(retry_days + DUE_MARGIN_DAYS + 1);
        let cutoff = now - Duration::days(keep);
        let mut report = EndReport::default();
        loop {
            let mut tx = self.services.store().begin().await?;
            let ended = tx
                .subscriptions()
                .end_expired(cutoff, now, END_BATCH)
                .await?;
            for item in &ended {
                tx.audit()
                    .record(&Actor::System.audit(
                        "subscription.ended",
                        audit_entity::SUBSCRIPTION,
                        Some(item.id.0),
                        serde_json::json!({
                            "client_id": item.client_id,
                            "kind": item.kind,
                            "expire_at": item.expire_at,
                            "ended_after_days": keep,
                        }),
                    ))
                    .await?;
            }
            tx.commit().await?;
            report.ended += ended.len();
            if ended.len() < usize::try_from(END_BATCH).unwrap_or(usize::MAX)
                || report.ended >= END_RUN_LIMIT
            {
                break;
            }
        }
        if report.ended > 0 {
            tracing::info!(ended = report.ended, "давно истёкшие подписки завершены");
        }
        Ok(report)
    }

    async fn renew(
        &self,
        subscription: &Subscription,
        now: DateTime<Utc>,
    ) -> ServiceResult<Attempt> {
        let settings = self.services.settings();
        let Some(expire_at) = subscription.mirror.expire_at else {
            return Ok(Attempt::Skipped);
        };
        let cycle = cycle_key(expire_at);
        if now > expire_at + Duration::days(i64::from(settings.autorenew.retry_days)) {
            self.disable(subscription, &cycle).await?;
            return Ok(Attempt::Skipped);
        }
        let window_start =
            expire_at - Duration::days(i64::from(settings.autorenew.charge_days_before));
        let day = u32::try_from((now - window_start).num_days().max(0)).unwrap_or(u32::MAX - 1);
        let number = day + 1;
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::AutoRenew, subscription.id.0).await?;
        let done = tx.autorenew().attempts(subscription.id, &cycle).await?;
        if done >= number {
            return Ok(Attempt::Skipped);
        }
        let current = tx.subscriptions().lock(subscription.id).await?;
        if !current.autorenew.enabled
            || current.ended_at.is_some()
            || current.mirror.expire_at != Some(expire_at)
            || crate::renewals::live_link(&mut tx, current.id)
                .await?
                .is_some()
        {
            return Ok(Attempt::Skipped);
        }
        for missed in 1..number {
            tx.autorenew()
                .record_attempt(
                    current.id,
                    &cycle,
                    missed,
                    AttemptResult::Skipped,
                    None,
                    Some("попытка пропущена"),
                )
                .await?;
        }
        let slot = Slot {
            subscription: &current,
            cycle,
            number,
        };
        let client = tx.clients().get(current.client_id).await?;
        if client.is_blocked() {
            return self
                .close(
                    tx,
                    &slot,
                    AttemptResult::Skipped,
                    None,
                    "клиент заблокирован",
                    Attempt::Skipped,
                )
                .await;
        }
        if current.mirror.status == Some(SubscriptionStatus::Disabled) {
            return self
                .close(
                    tx,
                    &slot,
                    AttemptResult::Skipped,
                    None,
                    "подписка заблокирована, автопродление не списывает",
                    Attempt::Skipped,
                )
                .await;
        }
        if let Some(reason) = self.unfinished(&mut tx, &current).await? {
            return self
                .close(
                    tx,
                    &slot,
                    AttemptResult::Skipped,
                    None,
                    &reason,
                    Attempt::Skipped,
                )
                .await;
        }
        let (order, total) = match self.price(&mut tx, &current).await {
            Ok(priced) => priced,
            Err(err) => {
                let reason = format!("цена не посчитана: {err}");
                return self
                    .close(
                        tx,
                        &slot,
                        AttemptResult::Failed,
                        None,
                        &reason,
                        Attempt::Failed,
                    )
                    .await;
            }
        };
        let balance = tx
            .ledger()
            .balance(current.client_id, Wallet::Main, settings.currency)
            .await?;
        if balance.minor >= total.minor {
            let (provider, debit) = if total.minor > 0 {
                (providers::BALANCE, Some(LedgerKind::AutoRenew))
            } else {
                (providers::MANUAL, None)
            };
            let paid = record_paid_order(
                &mut tx,
                &order,
                OrderChannel::Autorenew,
                provider,
                Some(AUTORENEW_METHOD),
                total.into(),
                debit,
            )
            .await?;
            let details = format!(
                "списано с баланса {}, платёж {}",
                format::money(total),
                paid.payment.public_id
            );
            return self
                .close(
                    tx,
                    &slot,
                    AttemptResult::Charged,
                    Some(paid.order.id),
                    &details,
                    Attempt::Charged,
                )
                .await;
        }
        if let Some(card) = self.saved_card(&mut tx, current.client_id).await? {
            return self.charge_card(tx, &slot, order, total, card).await;
        }
        let reminded = tx
            .autorenew()
            .mark_notified(
                current.client_id,
                "autorenew.reminder",
                &format!("{}:{}", current.id, slot.cycle),
            )
            .await?;
        if reminded {
            tx.jobs()
                .enqueue(
                    &JobPayload::Notify {
                        client_id: current.client_id,
                        template: "autorenew.reminder".into(),
                        params: serde_json::json!({
                            "subscription_id": current.id,
                            "expire_at": expire_at,
                            "amount": total,
                            "amount_text": format::money(total),
                            "balance": balance,
                            "balance_text": format::money(balance),
                            "missing": total.checked_sub(balance)?,
                            "balance_off": !self.services.payments().balance_enabled(),
                        }),
                    },
                    None,
                )
                .await?;
        }
        let details = format!(
            "на балансе {}, нужно {}",
            format::money(balance),
            format::money(total)
        );
        if reminded {
            self.close(
                tx,
                &slot,
                AttemptResult::Reminded,
                None,
                &details,
                Attempt::Reminded,
            )
            .await
        } else {
            self.close(
                tx,
                &slot,
                AttemptResult::Insufficient,
                None,
                &details,
                Attempt::Skipped,
            )
            .await
        }
    }

    async fn close(
        &self,
        mut tx: Tx,
        slot: &Slot<'_>,
        result: AttemptResult,
        order: Option<OrderId>,
        details: &str,
        attempt: Attempt,
    ) -> ServiceResult<Attempt> {
        if tx
            .autorenew()
            .record_attempt(
                slot.subscription.id,
                &slot.cycle,
                slot.number,
                result,
                order,
                Some(details),
            )
            .await?
        {
            tx.commit().await?;
            Ok(attempt)
        } else {
            tx.rollback().await?;
            Ok(Attempt::Skipped)
        }
    }

    async fn unfinished(
        &self,
        tx: &mut Tx,
        subscription: &Subscription,
    ) -> ServiceResult<Option<String>> {
        let page = tx
            .orders()
            .for_client(subscription.client_id, PageRequest::new(Some(50), None))
            .await?;
        for order in page
            .items
            .iter()
            .filter(|o| o.subscription_id == Some(subscription.id))
        {
            match order.status {
                OrderStatus::Paid => {
                    return Ok(Some(format!("заказ {} ещё не выдан", order.public_id)));
                }
                OrderStatus::Review => {
                    if crate::fulfillment::holds_autorenew(tx, order.id).await? {
                        return Ok(Some(format!("заказ {} ещё не выдан", order.public_id)));
                    }
                }
                OrderStatus::Pending => {
                    let payments = tx.payments().for_order(order.id).await?;
                    if payments.iter().any(|p| {
                        p.status == PaymentStatus::Pending
                            && p.method.as_deref() == Some(AUTORENEW_METHOD)
                    }) {
                        return Ok(Some(format!(
                            "ждём ответа платёжной системы по заказу {}",
                            order.public_id
                        )));
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }

    async fn price(
        &self,
        tx: &mut Tx,
        subscription: &Subscription,
    ) -> ServiceResult<(NewOrder, Money)> {
        let currency = self.services.settings().currency;
        let tariff_id = subscription.tariff_id.ok_or_else(|| {
            ServiceError::rejected("autorenew.no_tariff", "у подписки нет тарифа")
        })?;
        let mut tariff = tx.tariffs().get(tariff_id).await?;
        tariff.on_sale = true;
        let period = renewal_period(&tariff, subscription, currency).ok_or_else(|| {
            ServiceError::rejected(
                "autorenew.no_period",
                "у тарифа нет срока с ценой в валюте установки",
            )
        })?;
        let modules = self.services.modules();
        let selling = modules.is_enabled(ModuleId::Options);
        let extra_devices = if tariff
            .devices
            .check_extra(subscription.extra_devices, selling)
            .is_ok()
        {
            subscription.extra_devices
        } else {
            0
        };
        let kind = classify(
            tariff.category_id,
            Some(CurrentTariff {
                kind: subscription.kind,
                category_id: Some(tariff.category_id),
            }),
        );
        let hook = QuoteHook {
            client_id: subscription.client_id,
            tariff_id,
            period_id: period.id,
            order_kind: kind.order_kind(),
            currency,
            promo_code: None,
        };
        let adjustments = modules
            .quote_adjustments(&self.services.module_context(), &hook)
            .await?;
        let quote = quote(&QuoteRequest {
            currency,
            tariff: &tariff,
            period_id: period.id,
            extra_devices,
            sell_extra_devices: selling,
            adjustments: &adjustments,
        })?;
        let total = quote.total;
        Ok((
            NewOrder {
                client_id: subscription.client_id,
                subscription_id: Some(subscription.id),
                kind: kind.order_kind(),
                intent: PurchaseIntent::Subscription {
                    tariff_id,
                    period_id: period.id,
                    extra_devices,
                    target: Some(subscription.id),
                },
                quote,
                snapshot: Some(tariff.snapshot(period)),
            },
            total,
        ))
    }

    fn card_providers(&self) -> Vec<Arc<dyn PaymentProvider>> {
        let settings = self.services.settings();
        if !settings.autorenew.use_saved_card || self.services.secrets().is_none() {
            return Vec::new();
        }
        let registry = self.services.payments();
        let mut candidates: Vec<Arc<dyn PaymentProvider>> = registry
            .providers()
            .iter()
            .filter(|p| {
                p.kind() == ProviderKind::External
                    && p.supports_recurring()
                    && registry.is_available(p.id(), settings.currency)
            })
            .cloned()
            .collect();
        candidates.sort_by_key(|p| p.id() != providers::YOOKASSA);
        candidates
    }

    pub async fn has_saved_card(&self, client_id: ClientId) -> ServiceResult<bool> {
        let candidates = self.card_providers();
        if candidates.is_empty() {
            return Ok(false);
        }
        let mut conn = self.services.store().conn().await?;
        for provider in candidates {
            if conn
                .saved_methods()
                .active(client_id, provider.id())
                .await?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn saved_card(&self, tx: &mut Tx, client_id: ClientId) -> ServiceResult<Option<Card>> {
        let Some(secrets) = self.services.secrets() else {
            return Ok(None);
        };
        for provider in self.card_providers() {
            let Some(record) = tx.saved_methods().active(client_id, provider.id()).await? else {
                continue;
            };
            match secrets.open_string(
                &saved_method_context(client_id, provider.id()),
                &record.token,
            ) {
                Ok(token) => return Ok(Some(Card { provider, token })),
                Err(err) => {
                    tracing::warn!(client = %client_id, provider = provider.id(), error = %err, "сохранённая карта не расшифрована");
                }
            }
        }
        Ok(None)
    }

    fn hold_until(&self, subscription: &Subscription, now: DateTime<Utc>) -> DateTime<Utc> {
        let retry_days = i64::from(self.services.settings().autorenew.retry_days);
        let floor = now + Duration::days(HOLD_AFTER_RETRY_DAYS);
        subscription
            .mirror
            .expire_at
            .map(|at| at + Duration::days(retry_days + HOLD_AFTER_RETRY_DAYS))
            .map_or(floor, |hold| hold.max(floor))
    }

    fn charge_request(
        &self,
        payment: &Payment,
        order: &Order,
        client: ClientRef,
    ) -> InvoiceRequest {
        let settings = self.services.settings();
        let public_url = settings
            .public_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());
        InvoiceRequest {
            plan: None,
            payment_id: payment.id,
            payment_public_id: payment.public_id,
            order_public_id: order.public_id,
            amount: payment.amount,
            description: format!(
                "{} (автопродление)",
                describe(&settings.brand_name, &order.quote)
            ),
            client,
            return_url: None,
            webhook_url: public_url
                .map(|base| format!("{base}/api/webhooks/pay/{}", payment.provider.as_str()))
                .unwrap_or_default(),
            expires_in: Duration::minutes(i64::from(settings.invoice_ttl_minutes)),
            save_payment_method: false,
            method: None,
        }
    }

    async fn charge_card(
        &self,
        mut tx: Tx,
        slot: &Slot<'_>,
        order: NewOrder,
        total: Money,
        card: Card,
    ) -> ServiceResult<Attempt> {
        let subscription = slot.subscription;
        let now = self.services.now();
        let created = tx.orders().create(&order).await?;
        tx.orders()
            .set_channel(created.id, OrderChannel::Autorenew)
            .await?;
        let payment = tx
            .payments()
            .create(&NewPayment {
                order_id: created.id,
                client_id: created.client_id,
                provider: ProviderId::from(card.provider.id()),
                method: Some(AUTORENEW_METHOD.into()),
                amount: total.into(),
                expires_at: Some(self.hold_until(subscription, now)),
            })
            .await?;
        if !tx
            .autorenew()
            .record_attempt(
                subscription.id,
                &slot.cycle,
                slot.number,
                AttemptResult::Charged,
                Some(created.id),
                Some("списание с сохранённой карты"),
            )
            .await?
        {
            tx.rollback().await?;
            return Ok(Attempt::Skipped);
        }
        tx.jobs()
            .enqueue(
                &JobPayload::PollPayment {
                    payment_id: payment.id,
                },
                Some(now + Duration::seconds(POLL_DELAY_SECONDS)),
            )
            .await?;
        let client = tx.clients().get(subscription.client_id).await?;
        let client = client_ref(&mut tx, &client).await?;
        tx.commit().await?;
        let request = self.charge_request(&payment, &created, client);
        let config = self.services.payments().config(card.provider.id());
        match card
            .provider
            .charge_saved(&config, &request, &card.token)
            .await
        {
            Ok(outcome) => match self.settle_charge(&payment, outcome).await? {
                ChargeState::Paid | ChargeState::Pending => Ok(Attempt::Charged),
                ChargeState::Declined => {
                    self.notify_failed(
                        subscription,
                        &format!("{}:{}:{}", subscription.id, slot.cycle, slot.number),
                        total,
                    )
                    .await?;
                    Ok(Attempt::Failed)
                }
            },
            Err(err) if transient(&err) => {
                tracing::warn!(payment = %payment.id, error = %err, "исход списания с карты неизвестен, проверим повтором с тем же ключом");
                Ok(Attempt::Charged)
            }
            Err(err) => {
                tracing::warn!(payment = %payment.id, error = %err, "списание с сохранённой карты не прошло");
                self.services
                    .inbox()
                    .apply_outcome(
                        card.provider.id(),
                        &WebhookOutcome {
                            event_key: format!("autorenew:{}", payment.id),
                            payment: Some(PaymentRef::Payment(payment.id)),
                            provider_txid: None,
                            status: ProviderStatus::Failed,
                            amount: None,
                            saved_method: None,
                            reply: WebhookReply::ok(),
                            signal: None,
                        },
                    )
                    .await?;
                self.notify_failed(
                    subscription,
                    &format!("{}:{}:{}", subscription.id, slot.cycle, slot.number),
                    total,
                )
                .await?;
                Ok(Attempt::Failed)
            }
        }
    }

    async fn settle_charge(
        &self,
        payment: &Payment,
        outcome: WebhookOutcome,
    ) -> ServiceResult<ChargeState> {
        if outcome.status == ProviderStatus::Pending {
            if let Some(txid) = outcome
                .provider_txid
                .filter(|t| !t.trim().is_empty())
                .filter(|t| payment.provider_txid.as_deref() != Some(t.as_str()))
            {
                let mut conn = self.services.store().conn().await?;
                conn.payments()
                    .attach_invoice(
                        payment.id,
                        &InvoiceUpdate {
                            pay_url: None,
                            provider_txid: Some(txid),
                            expires_at: None,
                            payload: serde_json::json!({}),
                        },
                    )
                    .await?;
            }
            return Ok(ChargeState::Pending);
        }
        let status = outcome.status;
        self.services
            .inbox()
            .apply_outcome(
                payment.provider.as_str(),
                &WebhookOutcome {
                    payment: Some(PaymentRef::Payment(payment.id)),
                    ..outcome
                },
            )
            .await?;
        if status != ProviderStatus::Paid {
            return Ok(ChargeState::Declined);
        }
        self.flag_blocked_charge(payment.id).await?;
        Ok(ChargeState::Paid)
    }

    async fn flag_blocked_charge(&self, payment_id: PaymentId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let payment = conn.payments().get(payment_id).await?;
        if payment.status != PaymentStatus::Paid
            || !conn.clients().get(payment.client_id).await?.is_blocked()
        {
            return Ok(());
        }
        let order = conn.orders().get(payment.order_id).await?;
        if !matches!(order.status, OrderStatus::Paid | OrderStatus::Review)
            || last_audit_reason(&mut conn, payment.id, CHARGE_BLOCKED_AUDIT)
                .await?
                .is_some()
        {
            return Ok(());
        }
        let reason = "касса подтвердила автосписание у заблокированного клиента: \
                      заказ не выдаётся, оплата возвращается автоматически или ждёт ручной проверки";
        conn.audit()
            .record(&Actor::System.audit(
                CHARGE_BLOCKED_AUDIT,
                audit_entity::PAYMENT,
                Some(payment.id.0),
                charge_details(&payment, &order, reason),
            ))
            .await?;
        tracing::warn!(payment = %payment.id, order = %order.id, client = %payment.client_id, "автосписание заблокированного клиента подтверждено кассой, заказ не выдаётся");
        Ok(())
    }

    async fn note_unclear(&self, payment: &Payment, reason: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        if last_audit_reason(&mut conn, payment.id, CHARGE_UNCLEAR_AUDIT)
            .await?
            .as_deref()
            == Some(reason)
        {
            return Ok(());
        }
        let order = conn.orders().get(payment.order_id).await?;
        conn.audit()
            .record(&Actor::System.audit(
                CHARGE_UNCLEAR_AUDIT,
                audit_entity::PAYMENT,
                Some(payment.id.0),
                charge_details(payment, &order, reason),
            ))
            .await?;
        tracing::warn!(payment = %payment.id, order = %order.id, %reason, "автосписание ждёт ручной проверки");
        Ok(())
    }

    pub async fn check_charge(&self, payment_id: PaymentId) -> ServiceResult<Option<ChargeCheck>> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(payment_id).await?
        };
        if payment.method.as_deref() != Some(AUTORENEW_METHOD) {
            return Ok(None);
        }
        let registry = self.services.payments();
        let Some(provider) = registry
            .get(payment.provider.as_str())
            .filter(|p| p.kind() == ProviderKind::External)
        else {
            return Ok(None);
        };
        let checked = self.inspect_charge(&payment, &*provider).await?;
        if let ChargeCheck::Unclear(reason) = &checked {
            self.note_unclear(&payment, reason).await?;
        }
        Ok(Some(checked))
    }

    async fn inspect_charge(
        &self,
        payment: &Payment,
        provider: &dyn PaymentProvider,
    ) -> ServiceResult<ChargeCheck> {
        if payment.status != PaymentStatus::Pending {
            if payment.status == PaymentStatus::Paid {
                self.flag_blocked_charge(payment.id).await?;
            }
            return Ok(ChargeCheck::Settled);
        }
        let now = self.services.now();
        if payment.expires_at.is_some_and(|at| at <= now) {
            return Ok(ChargeCheck::Settled);
        }
        let config = self.services.payments().config(provider.id());
        let outcome = match payment.provider_txid.as_deref() {
            Some(txid) => match provider.fetch_status(&config, txid).await {
                Ok(Some(outcome)) => outcome,
                Ok(None)
                | Err(PaymentError::NotImplemented(_))
                | Err(PaymentError::Unsupported(_))
                | Err(PaymentError::NotConfigured(_)) => {
                    return Ok(ChargeCheck::Unclear(
                        "касса не отдаёт статус автосписания: проверьте платёж в её кабинете"
                            .into(),
                    ));
                }
                Err(err) if transient(&err) => return Ok(ChargeCheck::Pending),
                Err(err) => {
                    return Ok(ChargeCheck::Unclear(format!(
                        "статус автосписания не получен: {err}"
                    )));
                }
            },
            None => match self.repeat_charge(payment, provider, &config, now).await? {
                Repeat::Answered(outcome) => *outcome,
                Repeat::Pending => return Ok(ChargeCheck::Pending),
                Repeat::Unclear(reason) => return Ok(ChargeCheck::Unclear(reason)),
            },
        };
        match self.settle_charge(payment, outcome).await? {
            ChargeState::Pending => Ok(ChargeCheck::Pending),
            ChargeState::Paid => Ok(ChargeCheck::Settled),
            ChargeState::Declined => {
                self.notify_declined(payment).await?;
                Ok(ChargeCheck::Settled)
            }
        }
    }

    async fn repeat_charge(
        &self,
        payment: &Payment,
        provider: &dyn PaymentProvider,
        config: &ProviderConfig,
        now: DateTime<Utc>,
    ) -> ServiceResult<Repeat> {
        if now - payment.created_at > Duration::hours(REPEAT_WINDOW_HOURS) {
            return Ok(Repeat::Unclear(
                "исход автосписания неизвестен, а повтор запроса с тем же ключом уже небезопасен: проверьте платёж в кабинете кассы"
                    .into(),
            ));
        }
        let Some(secrets) = self.services.secrets() else {
            return Ok(Repeat::Unclear(
                "исход автосписания неизвестен: без SN_SECRET_KEY сохранённую карту не прочитать"
                    .into(),
            ));
        };
        let (order, client, record) = {
            let mut conn = self.services.store().conn().await?;
            let order = conn.orders().get(payment.order_id).await?;
            let client = conn.clients().get(payment.client_id).await?;
            if let Some(obstacle) = renewal_obstacle(&mut conn, &order, &client).await? {
                return Ok(Repeat::Unclear(format!(
                    "исход автосписания неизвестен, а {obstacle}: повторный запрос не отправлен, проверьте платёж в кабинете кассы"
                )));
            }
            let client = client_ref(&mut conn, &client).await?;
            let record = conn
                .saved_methods()
                .active(payment.client_id, provider.id())
                .await?;
            (order, client, record)
        };
        let Some(record) = record else {
            return Ok(Repeat::Unclear(
                "исход автосписания неизвестен, а сохранённой карты уже нет: проверьте платёж в кабинете кассы"
                    .into(),
            ));
        };
        let token = match secrets.open_string(
            &saved_method_context(payment.client_id, provider.id()),
            &record.token,
        ) {
            Ok(token) => token,
            Err(err) => {
                return Ok(Repeat::Unclear(format!(
                    "исход автосписания неизвестен, сохранённая карта не расшифрована: {err}"
                )));
            }
        };
        let request = self.charge_request(payment, &order, client);
        match provider.charge_saved(config, &request, &token).await {
            Ok(outcome) => Ok(Repeat::Answered(Box::new(outcome))),
            Err(err) if transient(&err) => Ok(Repeat::Pending),
            Err(err) => Ok(Repeat::Unclear(format!(
                "повторный запрос автосписания с тем же ключом отклонён, исход неизвестен: {err}"
            ))),
        }
    }

    async fn notify_declined(&self, payment: &Payment) -> ServiceResult<()> {
        let subscription = {
            let mut conn = self.services.store().conn().await?;
            if conn.clients().get(payment.client_id).await?.is_blocked() {
                return Ok(());
            }
            let order = conn.orders().get(payment.order_id).await?;
            match order.subscription_id {
                Some(id) => conn.subscriptions().get(id).await?,
                None => return Ok(()),
            }
        };
        let Some(total) = payment.amount.as_money() else {
            return Ok(());
        };
        self.notify_failed(
            &subscription,
            &format!("{}:payment:{}", subscription.id, payment.id),
            total,
        )
        .await
    }

    async fn notify_failed(
        &self,
        subscription: &Subscription,
        key: &str,
        total: Money,
    ) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        if tx
            .autorenew()
            .mark_notified(subscription.client_id, "autorenew.failed", key)
            .await?
        {
            tx.jobs()
                .enqueue(
                    &JobPayload::Notify {
                        client_id: subscription.client_id,
                        template: "autorenew.failed".into(),
                        params: serde_json::json!({
                            "subscription_id": subscription.id,
                            "expire_at": subscription.mirror.expire_at,
                            "amount": total,
                            "amount_text": format::money(total),
                            "balance_off": !self.services.payments().balance_enabled(),
                        }),
                    },
                    None,
                )
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn disable(&self, subscription: &Subscription, cycle: &str) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        tx.lock(LockSpace::AutoRenew, subscription.id.0).await?;
        let current = tx.subscriptions().lock(subscription.id).await?;
        if !current.autorenew.enabled {
            return Ok(());
        }
        tx.subscriptions()
            .set_autorenew(
                current.id,
                AutoRenewState {
                    enabled: false,
                    period_id: current.autorenew.period_id,
                },
            )
            .await?;
        if tx
            .autorenew()
            .mark_notified(
                current.client_id,
                "autorenew.disabled",
                &format!("{}:{cycle}", current.id),
            )
            .await?
        {
            tx.jobs()
                .enqueue(
                    &JobPayload::Notify {
                        client_id: current.client_id,
                        template: "autorenew.disabled".into(),
                        params: serde_json::json!({
                            "subscription_id": current.id,
                            "expire_at": current.mirror.expire_at,
                        }),
                    },
                    None,
                )
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
