use crate::error::{ServiceError, ServiceResult};
use crate::limits::{RATE_LIMITED, RateRule, Strike, UNKNOWN_ADDRESS, ip_key};
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId, LedgerEntryId, OrderId, PaymentId, RefundId};
use sn_domain::jobs::JobPayload;
use sn_domain::ledger::{LedgerKind, NewLedgerEntry, Wallet};
use sn_domain::money::{Amount, Money};
use sn_domain::order::{Order, OrderKind, OrderStatus};
use sn_domain::payment::{
    AmountCheck, NewPayment, NoticeKind, Payment, PaymentStatus, RefundMethod,
    RefundSubscriptionAction, check_paid_amount, providers,
};
use sn_modules::ModuleError;
use sn_payments::providers::{StarsPayment, StarsPreCheckout};
use sn_payments::{
    Buyer, OutcomeSignal, PaymentError, PaymentProvider, PaymentRef, PrecheckVerdict, ProviderKind,
    ProviderNotice, ProviderStatus, RefundRequest, RenewalRef, SavedMethod, WebhookOutcome,
    WebhookReply, WebhookRequest,
};
use sn_store::{
    ActorKind, AuditEntry, Db, InboxOutcome, NewInboundWebhook, NewPaymentNotice,
    NewProviderRefund, NewRefund, ProviderRefund, ProviderRefundStatus, RepeatAction, StatusChange,
    StoreError, Tx, audit_entity,
};
use std::net::IpAddr;
use uuid::Uuid;

pub(crate) mod blocked_refund;
mod bonus;
mod buyer;
mod journal;
mod masking;
mod refund_action;
mod refund_requests;
mod renewal;
#[cfg(test)]
mod tests;

pub use blocked_refund::{
    BLOCKED_REFUND_AUDIT, BLOCKED_REFUND_REASON, BLOCKED_REFUND_REQUESTED_AUDIT,
    BLOCKED_REFUND_TEMPLATE, BlockedRefund,
};
pub use bonus::RefundBonus;
pub use journal::{
    ATTENTION_HOURS, BodyView, Fact, JournalCard, JournalItem, JournalPage, JournalPaymentView,
    JournalQuery, OutcomeGroup, ProviderOption, RETRY_NOTICE_CLOSED, RETRY_REVERSED,
    RETRY_STALE_MINUTES, RETRY_UNCONFIRMED, Retention, RetryState, StepView, Tone,
    WebhookAttention, event_label, exact,
};
pub use masking::{MaskedBody, mask_body};
pub use refund_action::{
    REFUND_ACTION_ATTEMPTS, REFUND_ACTION_FAILED, RefundActionOutcome, RefundActionReview,
};
pub use refund_requests::{PENDING_REFUNDS_MAX, PendingRefundRequest};

fn digest_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

const STORED_HEADERS: [&str; 4] = ["content-type", "user-agent", "x-forwarded-for", "x-real-ip"];
const STORED_TEXT_LIMIT: usize = 300;

pub const MAX_WEBHOOK_BODY: usize = 64 * 1024;
pub const WEBHOOK_RETENTION_DAYS: i64 = 30;
pub const REJECTED_WEBHOOK_RETENTION_DAYS: i64 = 3;
pub const REPEAT_METHOD: &str = "repeat";
pub const REPEAT_CREDITED_TEMPLATE: &str = "payment.repeat_credited";
pub const REPEAT_REVIEW_TEMPLATE: &str = "payment.repeat_review";
pub const REPEAT_UNCONFIRMED: &str = "payment.repeat_unconfirmed";
const REFUND_REQUESTED_AUDIT: &str = "payment.refund_requested";
pub const PAYMENT_CREDITED_TEMPLATE: &str = "payment.credited";
pub const PAYMENT_REVIEW_TEMPLATE: &str = "payment.review";
pub const TOPUP_TEMPLATE: &str = "topup_done";
pub const OUTSIDE_REFUND_PREFIX: &str = "outside:";
const HOLD_REASON: &str = "платёжная система задержала оплату для проверки";
const NOTICE_TEXT_LIMIT: usize = 1000;
pub const LEGACY_CREDITED: &str = "Зачислено автоматически по счёту старой панели";

const PURGE_BATCH: i64 = 500;
const PURGE_ROUNDS: usize = 40;
const REFUND_POLL_DELAY_SECONDS: i64 = 60;

const REJECTED_WEBHOOKS: RateRule = RateRule {
    scope: "payment_webhook_rejected",
    limit: 30,
    window_seconds: 600,
};

const WEBHOOK_LOOKUPS: RateRule = RateRule {
    scope: "payment_webhook_lookup",
    limit: 10,
    window_seconds: 60,
};

fn address_key(ip: Option<IpAddr>) -> String {
    ip.map_or_else(|| UNKNOWN_ADDRESS.to_string(), ip_key)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn dedupe_key(request: &WebhookRequest) -> String {
    let digest = digest_hex(&request.body);
    match request
        .route_suffix
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
    {
        Some(kind) => match masking::route_event(Some(&kind.to_ascii_lowercase())) {
            Some(event) => format!("{event}:{digest}"),
            None => format!("route-{}:{digest}", clip(&digest_hex(kind.as_bytes()), 16)),
        },
        None => digest,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Received {
    Underpaid,
    Overpaid,
}

impl Received {
    const fn key(self) -> &'static str {
        match self {
            Received::Underpaid => "underpaid",
            Received::Overpaid => "overpaid",
        }
    }
}

fn rejection_outcome(code: &str) -> InboxOutcome {
    if code == "payment.amount_mismatch" {
        InboxOutcome::AmountMismatch
    } else {
        InboxOutcome::RejectedBusiness
    }
}

struct Settled {
    outcome: InboxOutcome,
    payment_id: Option<PaymentId>,
    error: Option<String>,
    failed: bool,
}

impl From<ServiceResult<AcceptResult>> for Settled {
    fn from(result: ServiceResult<AcceptResult>) -> Self {
        let settled = |outcome, payment_id, error| Settled {
            outcome,
            payment_id,
            error,
            failed: false,
        };
        match result {
            Ok(AcceptResult::Accepted { payment_id, .. })
            | Ok(AcceptResult::StatusChanged { payment_id, .. }) => {
                settled(InboxOutcome::Accepted, Some(payment_id), None)
            }
            Ok(AcceptResult::AlreadyApplied { payment_id }) => {
                settled(InboxOutcome::Duplicate, Some(payment_id), None)
            }
            Ok(AcceptResult::Ignored { reason }) => {
                settled(InboxOutcome::Ignored, None, Some(reason))
            }
            Err(ServiceError::NotFound(_))
            | Err(ServiceError::Store(sn_store::StoreError::NotFound(_))) => {
                settled(InboxOutcome::PaymentNotFound, None, None)
            }
            Err(ServiceError::Rejected { code, message }) => {
                settled(rejection_outcome(code), None, Some(message))
            }
            Err(err) => Settled {
                outcome: InboxOutcome::Error,
                payment_id: None,
                error: Some(err.to_string()),
                failed: true,
            },
        }
    }
}

fn stored_outcome(outcome: &WebhookOutcome) -> Option<serde_json::Value> {
    let mut copy = outcome.clone();
    if let Some(method) = copy.saved_method.as_mut() {
        method.token.clear();
    }
    serde_json::to_value(&copy).ok()
}

fn stored_headers(request: &WebhookRequest) -> serde_json::Value {
    serde_json::Value::Object(
        request
            .headers
            .iter()
            .filter(|(k, _)| STORED_HEADERS.contains(&k.to_ascii_lowercase().as_str()))
            .map(|(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    serde_json::Value::String(clip(v, STORED_TEXT_LIMIT)),
                )
            })
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundWebhook {
    pub provider: String,
    pub request: WebhookRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookResponse {
    pub reply: WebhookReply,
}

impl WebhookResponse {
    fn status(status: u16) -> Self {
        WebhookResponse {
            reply: WebhookReply::Empty { status },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcceptResult {
    Accepted {
        order_id: OrderId,
        payment_id: PaymentId,
    },
    AlreadyApplied {
        payment_id: PaymentId,
    },
    StatusChanged {
        payment_id: PaymentId,
        status: PaymentStatus,
    },
    Ignored {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollResult {
    Settled,
    Pending,
    Unsupported,
}

pub const CHAIN_LATE_HOURS: i64 = 24;
const CHAIN_WATCH_LIMIT: i64 = 200;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ChainWatchReport {
    pub checked: usize,
    pub settled: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RefundPollReport {
    pub settled: usize,
    pub waiting: usize,
    pub unsupported: usize,
    pub failed: usize,
    pub actions_done: usize,
    pub actions_retry: usize,
    pub actions_failed: usize,
}

fn keep_subscription() -> RefundSubscriptionAction {
    RefundSubscriptionAction::Keep
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundCommand {
    #[serde(default)]
    pub amount: Option<Amount>,
    pub method: RefundMethod,
    #[serde(default = "keep_subscription")]
    pub subscription_action: RefundSubscriptionAction,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub key: Option<Uuid>,
    #[serde(default)]
    pub outside: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundResult {
    pub refund_id: Option<RefundId>,
    pub request_id: Option<i64>,
    pub pending: bool,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub status: PaymentStatus,
    pub subscription_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundWay {
    pub available: bool,
    pub note: Option<String>,
}

impl RefundWay {
    fn open() -> Self {
        RefundWay {
            available: true,
            note: None,
        }
    }

    fn closed(note: impl Into<String>) -> Self {
        RefundWay {
            available: false,
            note: Some(note.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefundOptions {
    pub payment_id: PaymentId,
    pub status: PaymentStatus,
    pub provider: String,
    pub provider_title: String,
    pub amount: Amount,
    pub refunded: Amount,
    pub waiting: Amount,
    pub remaining: Amount,
    pub topup: bool,
    pub subscription: bool,
    pub days: u32,
    pub note: Option<String>,
    pub to_balance: RefundWay,
    pub via_provider: RefundWay,
    pub outside: RefundWay,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bonus: Option<RefundBonus>,
    pub gift: Option<GiftRecipient>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GiftRecipient {
    pub client_id: ClientId,
    pub subscription_id: Option<sn_domain::ids::SubscriptionId>,
}

struct RecordedRefund {
    refund_id: RefundId,
    status: PaymentStatus,
    order: Order,
    bonus_minor: i64,
}

enum RefundStep {
    Requested {
        request_id: i64,
        amount: Amount,
        provider_refund_id: Option<String>,
        hold: Option<LedgerEntryId>,
    },
    Recorded {
        recorded: Box<RecordedRefund>,
        amount: Amount,
    },
}

struct RefundSpec<'r> {
    amount: Amount,
    method: RefundMethod,
    action: RefundSubscriptionAction,
    reason: Option<&'r str>,
    admin: Option<AdminId>,
    provider_refund_id: Option<&'r str>,
    held_bonus: Option<i64>,
    request: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefundPoll {
    Settled,
    Waiting,
    Unsupported,
}

pub(crate) fn saved_method_context(client_id: ClientId, provider: &str) -> String {
    format!("saved_method:{client_id}:{provider}")
}

pub(crate) async fn credit_topup(
    tx: &mut Tx,
    order: &Order,
    paid: &Payment,
) -> ServiceResult<bool> {
    let amount = paid.amount.as_money().ok_or_else(|| {
        ServiceError::rejected("payment.topup_currency", "пополнение только в валюте")
    })?;
    let appended = tx
        .ledger()
        .append(&NewLedgerEntry {
            client_id: paid.client_id,
            wallet: Wallet::Main,
            amount,
            kind: LedgerKind::TopUp,
            order_id: Some(order.id),
            payment_id: Some(paid.id),
            admin_id: None,
            comment: None,
            idempotency_key: Some(format!("topup:payment:{}", paid.id)),
        })
        .await?;
    if !appended.duplicate {
        tx.events()
            .push(&DomainEvent::BalanceChanged {
                client_id: paid.client_id,
                wallet: Wallet::Main,
                kind: LedgerKind::TopUp,
                delta: amount,
            })
            .await?;
        let bonus = bonus::credit(tx, order, paid, amount).await?;
        if !tx.clients().get(paid.client_id).await?.is_blocked() {
            let mut params = serde_json::json!({
                "amount": amount,
                "order_id": order.id,
                "payment_id": paid.id,
            });
            if let Some(bonus) = bonus {
                params["bonus"] = serde_json::json!(bonus);
                params["total"] = serde_json::json!(amount.checked_add(bonus)?);
            }
            tx.jobs()
                .enqueue(
                    &JobPayload::Notify {
                        client_id: paid.client_id,
                        template: TOPUP_TEMPLATE.into(),
                        params,
                    },
                    None,
                )
                .await?;
        }
    }
    Ok(!appended.duplicate)
}

fn same_refund(stored: &str, event_key: &str) -> bool {
    stored == event_key
        || event_key
            .rsplit_once("refund:")
            .is_some_and(|(_, id)| !id.is_empty() && id == stored)
}

fn pending_requests(requests: &[ProviderRefund]) -> impl Iterator<Item = &ProviderRefund> {
    requests
        .iter()
        .filter(|r| r.status == ProviderRefundStatus::Pending)
}

fn pending_minor(requests: &[ProviderRefund]) -> i64 {
    pending_requests(requests).map(|r| r.amount.minor).sum()
}

fn matching_request<'r>(
    requests: &'r [ProviderRefund],
    event_key: &str,
    amount: Option<Amount>,
) -> Option<&'r ProviderRefund> {
    pending_requests(requests)
        .find(|r| {
            r.provider_refund_id
                .as_deref()
                .is_some_and(|id| same_refund(id, event_key))
        })
        .or_else(|| {
            amount.and_then(|amount| pending_requests(requests).find(|r| r.amount == amount))
        })
}

fn repeat_txid<'t>(payment: &Payment, incoming: Option<&'t str>) -> Option<&'t str> {
    let incoming = incoming?;
    let known = payment.provider_txid.as_deref()?;
    (payment.status.is_paid_like() && known != incoming).then_some(incoming)
}

fn credits_balance(order: &Order, repeat: Option<RepeatAction>) -> bool {
    match repeat {
        Some(RepeatAction::Balance) => true,
        Some(RepeatAction::Review) => false,
        None => order.kind == OrderKind::TopUp,
    }
}

fn amount_text(amount: Amount) -> String {
    match (amount.as_money(), amount.as_stars()) {
        (Some(money), _) => format::money(money),
        (None, Some(stars)) => format::stars(stars),
        (None, None) => amount.to_string(),
    }
}

fn amount_en(amount: Amount) -> String {
    let en = sn_domain::client::Locale::En;
    match (amount.as_money(), amount.as_stars()) {
        (Some(money), _) => format::money_in(money, en),
        (None, Some(stars)) => format::stars_in(stars, en),
        (None, None) => amount.to_string(),
    }
}

async fn refunded_minor(tx: &mut Tx, payment: PaymentId) -> ServiceResult<i64> {
    Ok(tx
        .refunds()
        .for_payment(payment)
        .await?
        .iter()
        .map(|r| r.amount.minor)
        .sum())
}

pub struct PaymentInbox<'a> {
    services: &'a Services,
}

async fn find_payment(
    tx: &mut Tx,
    provider: &str,
    reference: &PaymentRef,
) -> ServiceResult<PaymentId> {
    let id = match reference {
        PaymentRef::Payment(id) => *id,
        PaymentRef::PublicId(public_id) => tx.payments().by_public_id(*public_id).await?.id,
        PaymentRef::ProviderTxid(txid)
        | PaymentRef::Buyer(Buyer { txid, .. })
        | PaymentRef::Renewal(RenewalRef { txid, .. }) => {
            tx.payments()
                .by_provider_txid(provider, txid)
                .await?
                .ok_or(ServiceError::NotFound("платёж"))?
                .id
        }
    };
    Ok(id)
}

async fn lookup(
    db: &mut impl Db,
    provider: &str,
    reference: &PaymentRef,
) -> ServiceResult<Option<Payment>> {
    let found = match reference {
        PaymentRef::Payment(id) => db.payments().get(*id).await.map(Some),
        PaymentRef::PublicId(public_id) => db.payments().by_public_id(*public_id).await.map(Some),
        PaymentRef::ProviderTxid(txid)
        | PaymentRef::Buyer(Buyer { txid, .. })
        | PaymentRef::Renewal(RenewalRef { txid, .. }) => {
            db.payments().by_provider_txid(provider, txid).await
        }
    };
    match found {
        Ok(payment) => Ok(payment.filter(|p| p.provider.as_str() == provider)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn paid_elsewhere(tx: &mut Tx, order: OrderId, except: PaymentId) -> ServiceResult<bool> {
    Ok(tx
        .payments()
        .for_order(order)
        .await?
        .iter()
        .any(|p| p.id != except && p.status.is_paid_like()))
}

async fn locate(tx: &mut Tx, provider: &str, reference: &PaymentRef) -> ServiceResult<Payment> {
    let id = find_payment(tx, provider, reference).await?;
    let payment = tx.payments().lock(id).await?;
    if payment.provider.as_str() != provider {
        return Err(ServiceError::rejected(
            "payment.provider_mismatch",
            "платёж выставлен другой платёжной системой",
        ));
    }
    Ok(payment)
}

async fn locate_refunded(
    tx: &mut Tx,
    provider: &str,
    reference: &PaymentRef,
    txid: Option<&str>,
) -> ServiceResult<Payment> {
    let payment = locate(tx, provider, reference).await?;
    let Some(txid) = txid.filter(|t| {
        payment
            .provider_txid
            .as_deref()
            .is_some_and(|known| known != *t)
    }) else {
        return Ok(payment);
    };
    match tx.payments().by_provider_txid(provider, txid).await? {
        Some(other) if other.order_id == payment.order_id => {
            Ok(tx.payments().lock(other.id).await?)
        }
        _ => Ok(payment),
    }
}

async fn replay(tx: &mut Tx, payment: &Payment, key: Uuid) -> ServiceResult<Option<RefundResult>> {
    let reused = || {
        ServiceError::rejected(
            "refund.key_reused",
            "Этот ключ возврата уже использован для другого платежа",
        )
    };
    if let Some((refund_id, owner)) = tx.refunds().by_request_key(key).await? {
        if owner != payment.id {
            return Err(reused());
        }
        let amount = tx
            .refunds()
            .for_payment(payment.id)
            .await?
            .into_iter()
            .find(|r| r.id == refund_id)
            .map(|r| r.amount)
            .ok_or(ServiceError::NotFound("возврат"))?;
        return Ok(Some(RefundResult {
            refund_id: Some(refund_id),
            request_id: None,
            pending: false,
            payment_id: payment.id,
            amount,
            status: payment.status,
            subscription_error: None,
        }));
    }
    if let Some(request) = tx.payments().provider_refund_by_key(key).await? {
        if request.payment_id != payment.id {
            return Err(reused());
        }
        return Ok(Some(RefundResult {
            refund_id: request.refund_id,
            request_id: Some(request.id),
            pending: request.status == ProviderRefundStatus::Pending,
            payment_id: payment.id,
            amount: request.amount,
            status: payment.status,
            subscription_error: None,
        }));
    }
    Ok(None)
}

impl<'a> PaymentInbox<'a> {
    pub fn new(services: &'a Services) -> Self {
        PaymentInbox { services }
    }

    pub async fn handle(&self, hook: InboundWebhook) -> ServiceResult<WebhookResponse> {
        self.receive(hook, false).await
    }

    pub async fn handle_legacy(&self, hook: InboundWebhook) -> ServiceResult<WebhookResponse> {
        self.receive(hook, true).await
    }

    async fn mark_legacy(&self, source: &str, dedupe_key: &str, path: bool, invoice: Option<i64>) {
        if !path && invoice.is_none() {
            return;
        }
        let marked = match self.services.store().conn().await {
            Ok(mut conn) => conn
                .webhooks()
                .mark_legacy(source, dedupe_key, path, invoice)
                .await
                .map_err(ServiceError::from),
            Err(err) => Err(err.into()),
        };
        if let Err(err) = marked {
            tracing::warn!(source, error = %err, "пометка старого адреса в журнале не записана");
        }
    }

    async fn receive(
        &self,
        hook: InboundWebhook,
        legacy_path: bool,
    ) -> ServiceResult<WebhookResponse> {
        let registry = self.services.payments();
        let Some(provider) = registry.get(&hook.provider) else {
            return Ok(WebhookResponse::status(404));
        };
        if hook.request.body.len() > MAX_WEBHOOK_BODY {
            return Ok(WebhookResponse::status(413));
        }
        let dedupe_key = dedupe_key(&hook.request);
        if !registry.accepts_webhooks(provider.id()) {
            let reason =
                if provider.kind() == ProviderKind::External && provider.accepts_http_webhooks() {
                    "платёжная система не настроена"
                } else {
                    "платёжная система не принимает уведомления по HTTP"
                };
            self.note_rejection(
                provider.id(),
                &hook.request,
                &dedupe_key,
                InboxOutcome::NotAccepted,
                reason,
            )
            .await;
            self.mark_legacy(
                provider.id(),
                &format!("rejected:{dedupe_key}"),
                legacy_path,
                None,
            )
            .await;
            return Ok(WebhookResponse::status(404));
        }
        let config = registry.config(provider.id());
        let gate = if provider.verifies_webhooks_by_api() {
            match self.lookup_gate(provider.id(), &hook.request).await? {
                Some(strike) => Some(strike),
                None => return Ok(WebhookResponse::status(429)),
            }
        } else {
            None
        };
        let parsed = match provider.parse_webhook(&config, &hook.request).await {
            Ok(parsed) => {
                if let Some(strike) = &gate
                    && let Err(err) = self.services.limits().forgive(strike).await
                {
                    tracing::warn!(provider = provider.id(), error = %err, "отметка проверки вебхука не снята");
                }
                parsed
            }
            Err(err) => {
                let (outcome, status) = match &err {
                    PaymentError::BadSignature => (InboxOutcome::RejectedSignature, 401),
                    PaymentError::BadPayload(_) => (InboxOutcome::RejectedPayload, 400),
                    _ => (InboxOutcome::Error, 503),
                };
                self.note_rejection(
                    provider.id(),
                    &hook.request,
                    &dedupe_key,
                    outcome,
                    &err.to_string(),
                )
                .await;
                self.mark_legacy(
                    provider.id(),
                    &format!("rejected:{dedupe_key}"),
                    legacy_path,
                    None,
                )
                .await;
                return Ok(WebhookResponse::status(status));
            }
        };
        let parsed = match &parsed.signal {
            Some(OutcomeSignal::Precheck { account }) => {
                match self
                    .precheck(provider.id(), &parsed, account.as_deref())
                    .await
                {
                    Ok(verdict) => WebhookOutcome {
                        reply: provider.precheck_reply(verdict),
                        ..parsed
                    },
                    Err(err) => {
                        tracing::warn!(provider = provider.id(), error = %err, "проверка платежа перед списанием не прошла");
                        return Ok(WebhookResponse::status(503));
                    }
                }
            }
            _ => parsed,
        };
        let prepared = masking::prepare(&hook.request.body, hook.request.route_suffix.as_deref());
        let record = {
            let mut conn = self.services.store().conn().await?;
            conn.webhooks()
                .record(&NewInboundWebhook {
                    source: provider.id().to_string(),
                    dedupe_key: dedupe_key.clone(),
                    remote_ip: hook.request.remote_ip.map(|ip| ip.to_string()),
                    headers: stored_headers(&hook.request),
                    body: Some(prepared.body),
                    parsed: stored_outcome(&parsed),
                    event: prepared.event,
                })
                .await?
        };
        let record_key = dedupe_key;
        if record.duplicate && record.outcome.is_final() {
            self.mark_legacy(provider.id(), &record_key, legacy_path, None)
                .await;
            return Ok(WebhookResponse {
                reply: parsed.reply,
            });
        }
        if record.duplicate {
            let stale_before = self.services.now() - Duration::minutes(RETRY_STALE_MINUTES);
            let claimed = {
                let mut conn = self.services.store().conn().await?;
                conn.webhooks().claim(record.id, stale_before).await?
            };
            if !claimed {
                tracing::info!(
                    provider = provider.id(),
                    webhook = record.id,
                    "уведомление уже обрабатывается, платёжка пришлёт его ещё раз"
                );
                self.mark_legacy(provider.id(), &record_key, legacy_path, None)
                    .await;
                return Ok(WebhookResponse::status(503));
            }
        }
        if !record.duplicate
            && let Err(err) = self.purge_batch(PURGE_BATCH).await
        {
            tracing::warn!(error = %err, "старые записи вебхуков не удалены");
        }
        let mut settled = Settled::from(self.apply_outcome(provider.id(), &parsed).await);
        let legacy_invoice = self
            .settle_legacy(provider.id(), &parsed, &hook.request, &mut settled, None)
            .await;
        if settled.payment_id.is_none() && settled.outcome != InboxOutcome::PaymentNotFound {
            settled.payment_id = self.linked_payment(provider.id(), &parsed).await;
        }
        let response = if settled.failed {
            WebhookResponse::status(503)
        } else {
            WebhookResponse {
                reply: parsed.reply,
            }
        };
        let mut conn = self.services.store().conn().await?;
        conn.webhooks()
            .finish(
                record.id,
                settled.outcome,
                settled.payment_id,
                settled.error.as_deref(),
                None,
            )
            .await?;
        drop(conn);
        self.mark_legacy(provider.id(), &record_key, legacy_path, legacy_invoice)
            .await;
        Ok(response)
    }

    async fn settle_legacy(
        &self,
        provider: &str,
        parsed: &WebhookOutcome,
        request: &WebhookRequest,
        settled: &mut Settled,
        admin: Option<AdminId>,
    ) -> Option<i64> {
        if !matches!(
            settled.outcome,
            InboxOutcome::PaymentNotFound | InboxOutcome::Ignored
        ) {
            return None;
        }
        let series = match &parsed.payment {
            Some(PaymentRef::Renewal(renewal)) => {
                if settled.error.as_deref() != Some(renewal::UNKNOWN_RENEWAL) {
                    return None;
                }
                Some(renewal.series.clone())
            }
            _ => None,
        };
        match crate::legacy::invoices::settle(self.services, provider, parsed, request).await {
            Ok(Some(found)) => {
                if let Some(series) = &series {
                    self.note_legacy_contract(found.invoice, series).await;
                }
                let again = WebhookOutcome {
                    payment: Some(PaymentRef::Payment(found.payment)),
                    ..parsed.clone()
                };
                *settled = Settled::from(self.apply_outcome(provider, &again).await);
                settled.payment_id = settled.payment_id.or(Some(found.payment));
                if matches!(
                    settled.outcome,
                    InboxOutcome::Accepted | InboxOutcome::Duplicate
                ) {
                    self.close_legacy_credit(provider, &parsed.event_key, admin)
                        .await;
                }
                Some(found.invoice)
            }
            Ok(None) => None,
            Err(err) => {
                tracing::error!(provider, error = %err, "счёт старой панели не проведён");
                *settled = Settled {
                    outcome: InboxOutcome::Error,
                    payment_id: None,
                    error: Some(format!("счёт старой панели не проведён: {err}")),
                    failed: true,
                };
                None
            }
        }
    }

    async fn note_legacy_contract(&self, invoice: i64, series: &str) {
        let noted = match self.services.store().conn().await {
            Ok(mut conn) => {
                crate::renewals::legacy::note_legacy_charge(
                    &mut conn,
                    invoice,
                    series,
                    self.services.now(),
                )
                .await
            }
            Err(err) => Err(err.into()),
        };
        if let Err(err) = noted {
            tracing::warn!(legacy_invoice = invoice, error = %err, "договор Lava.top старой панели не отмечен для проверки");
        }
    }

    async fn close_legacy_credit(&self, provider: &str, event_key: &str, admin: Option<AdminId>) {
        let closed = match self.services.store().conn().await {
            Ok(mut conn) => conn
                .payment_notices()
                .close_credit_event(
                    provider,
                    event_key,
                    admin,
                    LEGACY_CREDITED,
                    self.services.now(),
                )
                .await
                .map_err(ServiceError::from),
            Err(err) => Err(err.into()),
        };
        if let Err(err) = closed {
            tracing::error!(provider, event = event_key, error = %err, "оповещение о ручном зачислении не закрыто, хотя списание проведено по счёту старой панели");
        }
    }

    async fn linked_payment(&self, provider: &str, outcome: &WebhookOutcome) -> Option<PaymentId> {
        let reference = outcome.payment.as_ref()?;
        let mut conn = self.services.store().conn().await.ok()?;
        match lookup(&mut conn, provider, reference).await {
            Ok(found) => found.map(|p| p.id),
            Err(err) => {
                tracing::debug!(provider, error = %err, "платёж из уведомления не найден для журнала");
                None
            }
        }
    }

    async fn lookup_gate(
        &self,
        provider: &str,
        request: &WebhookRequest,
    ) -> ServiceResult<Option<Strike>> {
        let identity = format!("{provider}|{}", address_key(request.remote_ip));
        match self
            .services
            .limits()
            .strike(
                WEBHOOK_LOOKUPS,
                &identity,
                "Слишком много уведомлений, не прошедших проверку.",
            )
            .await
        {
            Ok(strike) => Ok(Some(strike)),
            Err(ServiceError::Rejected { code, .. }) if code == RATE_LIMITED => {
                tracing::debug!(
                    provider,
                    address = %address_key(request.remote_ip),
                    "вебхук отклонён до проверки: слишком много непроверенных уведомлений с этого адреса"
                );
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    async fn note_rejection(
        &self,
        provider: &str,
        request: &WebhookRequest,
        digest: &str,
        outcome: InboxOutcome,
        reason: &str,
    ) {
        let ip = request.remote_ip.map(|ip| ip.to_string());
        let identity = format!("{provider}|{}", address_key(request.remote_ip));
        match self
            .services
            .limits()
            .hit(REJECTED_WEBHOOKS, &identity)
            .await
        {
            Ok(_) => {}
            Err(ServiceError::Rejected { code, .. }) if code == RATE_LIMITED => {
                tracing::debug!(
                    provider,
                    ip = ip.as_deref(),
                    outcome = outcome.as_str(),
                    "отказ вебхука не записан: слишком много отказов с этого адреса"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(provider, error = %err, "отказ вебхука не записан");
                return;
            }
        }
        tracing::warn!(
            provider,
            ip = ip.as_deref(),
            outcome = outcome.as_str(),
            error = reason,
            body_sha256 = digest,
            body_len = request.body.len(),
            "вебхук отклонён"
        );
        let error = clip(reason, STORED_TEXT_LIMIT);
        let record = NewInboundWebhook {
            source: provider.to_string(),
            dedupe_key: format!("rejected:{digest}"),
            remote_ip: ip,
            headers: stored_headers(request),
            body: None,
            parsed: None,
            event: masking::route_event(request.route_suffix.as_deref()),
        };
        let stored = match self.services.store().conn().await {
            Ok(mut conn) => conn
                .webhooks()
                .record_rejected(&record, outcome, Some(&error))
                .await
                .map(|_| ()),
            Err(err) => Err(err),
        };
        if let Err(err) = stored {
            tracing::warn!(provider, error = %err, "отказ вебхука не записан");
        }
    }

    async fn purge_batch(&self, limit: i64) -> ServiceResult<u64> {
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .webhooks()
            .purge(
                now - Duration::days(WEBHOOK_RETENTION_DAYS),
                now - Duration::days(REJECTED_WEBHOOK_RETENTION_DAYS),
                limit,
            )
            .await?)
    }

    pub async fn purge_webhooks(&self) -> ServiceResult<u64> {
        let mut total = 0u64;
        for _ in 0..PURGE_ROUNDS {
            let purged = self.purge_batch(PURGE_BATCH).await?;
            total = total.saturating_add(purged);
            if purged == 0 {
                break;
            }
        }
        Ok(total)
    }

    pub async fn apply_outcome(
        &self,
        provider: &str,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        if let Some(notice) = outcome.notice()
            && outcome.status != ProviderStatus::Held
        {
            self.note_event(provider, outcome, notice).await?;
        }
        if let Some(event) = outcome.renewal_event() {
            return self.accept_renewal_event(provider, event).await;
        }
        let Some(reference) = &outcome.payment else {
            return Ok(AcceptResult::Ignored {
                reason: "событие без нашего платежа".into(),
            });
        };
        if let PaymentRef::Buyer(buyer) = reference
            && outcome.status == ProviderStatus::Paid
        {
            return self.accept_buyer(provider, buyer, outcome).await;
        }
        if let PaymentRef::Renewal(renewal) = reference {
            if outcome.status == ProviderStatus::Paid {
                return self.accept_renewal(provider, renewal, outcome).await;
            }
            return Ok(AcceptResult::Ignored {
                reason: "продление ещё не оплачено".into(),
            });
        }
        match outcome.status {
            ProviderStatus::Pending
                if outcome
                    .notice()
                    .is_some_and(|n| n.kind == NoticeKind::RefundReported) =>
            {
                self.accept_failure(provider, reference, outcome, PaymentStatus::Canceled)
                    .await
            }
            ProviderStatus::Pending => {
                if let (PaymentRef::Payment(id), Some(txid)) =
                    (reference, outcome.provider_txid.as_deref())
                    && let Err(err) = self.track_pending(*id, txid).await
                {
                    tracing::warn!(payment = %id, error = %err, "номер транзакции ожидающего платежа не сохранён");
                }
                Ok(AcceptResult::Ignored {
                    reason: "платёж ещё не завершён".into(),
                })
            }
            ProviderStatus::Paid => self.accept_paid(provider, reference, outcome, None).await,
            ProviderStatus::Overpaid { received } => {
                self.accept_paid(provider, reference, outcome, Some(received))
                    .await
            }
            ProviderStatus::Underpaid { received } => {
                self.accept_underpaid(provider, reference, outcome, received)
                    .await
            }
            ProviderStatus::Held => self.accept_held(provider, reference, outcome).await,
            ProviderStatus::Failed => {
                self.accept_failure(provider, reference, outcome, PaymentStatus::Failed)
                    .await
            }
            ProviderStatus::Canceled => {
                self.accept_failure(provider, reference, outcome, PaymentStatus::Canceled)
                    .await
            }
            ProviderStatus::Refunded => {
                let result = self
                    .accept_refund(provider, reference, outcome, None)
                    .await?;
                self.renewal_after_refund(&result).await;
                Ok(result)
            }
            ProviderStatus::PartiallyRefunded { refunded } => {
                let result = self
                    .accept_refund(provider, reference, outcome, Some(refunded))
                    .await?;
                self.renewal_after_refund(&result).await;
                Ok(result)
            }
            ProviderStatus::RefundDeclined => {
                self.decline_refund(provider, reference, outcome).await
            }
        }
    }

    async fn precheck(
        &self,
        provider: &str,
        outcome: &WebhookOutcome,
        account: Option<&str>,
    ) -> ServiceResult<PrecheckVerdict> {
        let mut conn = self.services.store().conn().await?;
        let Some(reference) = &outcome.payment else {
            return Ok(PrecheckVerdict::UnknownInvoice);
        };
        let Some(payment) = lookup(&mut conn, provider, reference).await? else {
            drop(conn);
            return Ok(
                match crate::legacy::invoices::precheck(self.services, provider, outcome).await? {
                    Some(true) => PrecheckVerdict::Accept,
                    Some(false) => PrecheckVerdict::WrongAmount,
                    None => PrecheckVerdict::UnknownInvoice,
                },
            );
        };
        let client = conn.clients().get(payment.client_id).await?;
        if account.is_some_and(|account| Uuid::parse_str(account).ok() != Some(client.public_id)) {
            return Ok(PrecheckVerdict::WrongAccount);
        }
        if outcome.amount != Some(payment.amount) {
            return Ok(PrecheckVerdict::WrongAmount);
        }
        if client.is_blocked() {
            return Ok(PrecheckVerdict::Rejected);
        }
        let open = payment.expires_at.is_none_or(|at| at > self.services.now());
        Ok(match payment.status {
            PaymentStatus::Pending | PaymentStatus::Failed if open => PrecheckVerdict::Accept,
            PaymentStatus::Pending | PaymentStatus::Failed | PaymentStatus::Canceled if !open => {
                PrecheckVerdict::Expired
            }
            _ => PrecheckVerdict::Rejected,
        })
    }

    async fn note_event(
        &self,
        provider: &str,
        outcome: &WebhookOutcome,
        notice: &ProviderNotice,
    ) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let payment = match &outcome.payment {
            Some(reference) => lookup(&mut conn, provider, reference).await?,
            None => None,
        };
        let recorded = conn
            .payment_notices()
            .record(&NewPaymentNotice {
                provider: provider.to_string(),
                event_key: outcome.event_key.clone(),
                kind: notice.kind,
                payment_id: payment.as_ref().map(|p| p.id),
                client_id: payment.as_ref().map(|p| p.client_id),
                amount: outcome.amount.or(payment.as_ref().map(|p| p.amount)),
                text: clip(&notice.text, NOTICE_TEXT_LIMIT),
            })
            .await?;
        if recorded.is_some() {
            tracing::warn!(
                provider,
                event = %outcome.event_key,
                kind = notice.kind.as_str(),
                payment = ?payment.as_ref().map(|p| p.id),
                "платёжная система сообщила о событии, которое нужно проверить вручную"
            );
        }
        if notice.kind == NoticeKind::Chargeback
            && let Some(payment) = &payment
            && let Err(err) = self
                .services
                .renewals()
                .cancel_for_payment(payment.id, crate::renewals::CancelSource::Chargeback)
                .await
        {
            tracing::warn!(payment = %payment.id, error = %err, "автопродление после чарджбэка не отменено, повторит сверка");
        }
        if notice.kind == NoticeKind::Chargeback
            && let Some(payment) = &payment
            && let Some(grant) = refund_action::gift_grant(&mut conn, payment.order_id).await?
        {
            conn.payment_notices()
                .record(&NewPaymentNotice {
                    provider: provider.to_string(),
                    event_key: format!("gift_chargeback:{}", outcome.event_key),
                    kind: NoticeKind::RefundReported,
                    payment_id: Some(payment.id),
                    client_id: Some(payment.client_id),
                    amount: outcome.amount.or(Some(payment.amount)),
                    text: format!(
                        "Чарджбэк по подарку, который уже активировал клиент #{}: его подписка не изменена. \
                         Решите, отключить ли её или укоротить",
                        grant.client_id
                    ),
                })
                .await?;
        }
        Ok(())
    }

    async fn accept_held(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate(&mut tx, provider, reference).await?;
        if payment.status != PaymentStatus::Pending {
            drop(tx);
            if let Some(notice) = outcome.notice() {
                self.note_event(provider, outcome, notice).await?;
            }
            return Ok(AcceptResult::Ignored {
                reason: format!("платёж уже {}", payment.status.as_str()),
            });
        }
        if let Some(txid) = outcome.provider_txid.as_deref() {
            tx.payments().remember_txid(payment.id, txid).await?;
        }
        tx.payments().hold(payment.id).await?;
        let order = tx.orders().lock(payment.order_id).await?;
        if order.status == OrderStatus::Pending {
            let reason = outcome.notice().map_or_else(
                || HOLD_REASON.to_string(),
                |n| clip(&n.text, NOTICE_TEXT_LIMIT),
            );
            tx.orders().mark_review(order.id, &reason).await?;
            tx.audit()
                .record(&AuditEntry {
                    actor_kind: ActorKind::System,
                    actor_id: None,
                    action: "order.review".into(),
                    entity_kind: Some(audit_entity::ORDER.into()),
                    entity_id: Some(order.id.0),
                    details: serde_json::json!({
                        "payment_id": payment.id,
                        "amount": payment.amount,
                        "reason": reason,
                        "held": true,
                    }),
                    ip: None,
                })
                .await?;
            self.notify(
                &mut tx,
                payment.client_id,
                PAYMENT_REVIEW_TEMPLATE,
                (
                    format!(
                        "Оплата {} получена, но платёжная система задержала её для проверки. \
                         Заказ ждёт ручной проверки, мы свяжемся с вами.",
                        amount_text(payment.amount)
                    ),
                    format!(
                        "Payment of {} received, but the payment system held it for a check. \
                         Your order is waiting for a manual review, we'll get in touch.",
                        amount_en(payment.amount)
                    ),
                ),
                &payment,
                &order,
            )
            .await?;
            tracing::warn!(payment = %payment.id, order = %order.id, "платёжная система задержала оплату, заказ ждёт ручной проверки");
        }
        tx.commit().await?;
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: PaymentStatus::Pending,
        })
    }

    async fn notify(
        &self,
        tx: &mut Tx,
        client: ClientId,
        template: &str,
        (text, text_en): (String, String),
        payment: &Payment,
        order: &Order,
    ) -> ServiceResult<()> {
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id: client,
                    template: template.into(),
                    params: serde_json::json!({
                        "text": text,
                        "text_en": text_en,
                        "amount": payment.amount,
                        "payment_id": payment.id,
                        "order_id": order.id,
                    }),
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn accept_underpaid(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
        received: Amount,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate(&mut tx, provider, reference).await?;
        if payment.status.is_paid_like() {
            return Ok(AcceptResult::Ignored {
                reason: format!("платёж уже {}", payment.status.as_str()),
            });
        }
        let change = match tx
            .payments()
            .transition(
                payment.id,
                PaymentStatus::Failed,
                outcome.provider_txid.as_deref(),
                Some("оплата пришла не полностью"),
            )
            .await
        {
            Ok(change) => change,
            Err(StoreError::Invalid(_)) => StatusChange::Unchanged(payment.clone()),
            Err(StoreError::Conflict { message, .. }) => {
                return Err(ServiceError::rejected("payment.txid_mismatch", message));
            }
            Err(err) => return Err(err.into()),
        };
        let order = tx.orders().lock(payment.order_id).await?;
        let held = order.status == OrderStatus::Review
            && !paid_elsewhere(&mut tx, order.id, payment.id).await?;
        let order = if order.status == OrderStatus::Pending || held {
            let (canceled, changed) = tx
                .orders()
                .transition(order.id, OrderStatus::Canceled)
                .await?;
            if changed {
                let ctx = self.services.module_context();
                self.services
                    .modules()
                    .order_canceled(&ctx, &mut tx, &canceled)
                    .await?;
            }
            canceled
        } else {
            order
        };
        self.credit_received(
            &mut tx,
            provider,
            &order,
            &payment,
            received,
            Received::Underpaid,
        )
        .await?;
        tx.commit().await?;
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: change.payment().status,
        })
    }

    async fn credit_received(
        &self,
        tx: &mut Tx,
        provider: &str,
        order: &Order,
        payment: &Payment,
        amount: Amount,
        why: Received,
    ) -> ServiceResult<()> {
        if amount.minor <= 0 {
            return Ok(());
        }
        let currency = self.services.settings().currency;
        let shown = amount_text(amount);
        let expected = amount_text(payment.amount);
        let shown_en = amount_en(amount);
        let expected_en = amount_en(payment.amount);
        let key = format!("{}:payment:{}", why.key(), payment.id);
        let details = serde_json::json!({
            "order_id": order.id,
            "amount": amount,
            "expected": payment.amount,
            "provider_txid": payment.provider_txid,
        });
        match amount.as_money().filter(|m| m.currency == currency) {
            Some(money) => {
                let appended = tx
                    .ledger()
                    .append(&NewLedgerEntry {
                        client_id: payment.client_id,
                        wallet: Wallet::Main,
                        amount: money,
                        kind: LedgerKind::TopUp,
                        order_id: Some(order.id),
                        payment_id: Some(payment.id),
                        admin_id: None,
                        comment: Some(match why {
                            Received::Underpaid => format!(
                                "Оплата заказа {} пришла не полностью: получено {shown} из {expected}, зачислено на баланс",
                                order.public_id
                            ),
                            Received::Overpaid => format!(
                                "Излишек оплаты заказа {}: зачислено на баланс",
                                order.public_id
                            ),
                        }),
                        idempotency_key: Some(key),
                    })
                    .await?;
                if appended.duplicate {
                    return Ok(());
                }
                tx.events()
                    .push(&DomainEvent::BalanceChanged {
                        client_id: payment.client_id,
                        wallet: Wallet::Main,
                        kind: LedgerKind::TopUp,
                        delta: money,
                    })
                    .await?;
                tx.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::System,
                        actor_id: None,
                        action: format!("payment.{}_credited", why.key()),
                        entity_kind: Some("payment".into()),
                        entity_id: Some(payment.id.0),
                        details,
                        ip: None,
                    })
                    .await?;
                let text = match why {
                    Received::Underpaid => (
                        format!(
                            "Оплата пришла не полностью: получено {shown} из {expected}. \
                             Заказ не оформлен, полученные {shown} зачислены на баланс."
                        ),
                        format!(
                            "The payment didn't arrive in full: {shown_en} of {expected_en}. \
                             The order wasn't placed, the {shown_en} received is on your balance."
                        ),
                    ),
                    Received::Overpaid => (
                        format!("Оплата пришла с излишком: лишние {shown} зачислены на баланс."),
                        format!(
                            "You paid more than needed: the extra {shown_en} is on your balance."
                        ),
                    ),
                };
                self.notify(
                    tx,
                    payment.client_id,
                    PAYMENT_CREDITED_TEMPLATE,
                    text,
                    payment,
                    order,
                )
                .await?;
                tracing::info!(payment = %payment.id, amount = %shown, reason = why.key(), "полученные деньги зачислены на баланс");
            }
            None => {
                let recorded = tx
                    .payment_notices()
                    .record(&NewPaymentNotice {
                        provider: provider.to_string(),
                        event_key: key,
                        kind: NoticeKind::ManualCredit,
                        payment_id: Some(payment.id),
                        client_id: Some(payment.client_id),
                        amount: Some(amount),
                        text: match why {
                            Received::Underpaid => format!(
                                "Оплата пришла не полностью: получено {shown} из {expected}, заказ отменён. \
                                 Деньги в другой валюте, чем валюта магазина ({}), поэтому на баланс \
                                 сами не зачислились: зачислите их вручную или верните клиенту",
                                currency.code()
                            ),
                            Received::Overpaid => format!(
                                "Оплата пришла с излишком {shown}. Излишек в другой валюте, чем \
                                 валюта магазина ({}), поэтому на баланс сам не зачислился: \
                                 зачислите его вручную или верните клиенту",
                                currency.code()
                            ),
                        },
                    })
                    .await?;
                if recorded.is_none() {
                    return Ok(());
                }
                tx.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::System,
                        actor_id: None,
                        action: format!("payment.{}_review", why.key()),
                        entity_kind: Some("payment".into()),
                        entity_id: Some(payment.id.0),
                        details,
                        ip: None,
                    })
                    .await?;
                let text = match why {
                    Received::Underpaid => (
                        format!(
                            "Оплата пришла не полностью: получено {shown} из {expected}. \
                             Заказ не оформлен. Мы проверим платёж вручную и свяжемся с вами."
                        ),
                        format!(
                            "The payment didn't arrive in full: {shown_en} of {expected_en}. \
                             The order wasn't placed. We'll check the payment by hand and get in touch."
                        ),
                    ),
                    Received::Overpaid => (
                        format!(
                            "Оплата пришла с излишком {shown}. Мы проверим платёж вручную и свяжемся с вами."
                        ),
                        format!(
                            "You paid {shown_en} more than needed. We'll check the payment by hand and get in touch."
                        ),
                    ),
                };
                self.notify(
                    tx,
                    payment.client_id,
                    PAYMENT_REVIEW_TEMPLATE,
                    text,
                    payment,
                    order,
                )
                .await?;
                tracing::warn!(payment = %payment.id, amount = %shown, reason = why.key(), "полученные деньги ждут ручного зачисления");
            }
        }
        Ok(())
    }

    async fn revoke_method(&self, tx: &mut Tx, payment: &Payment) -> ServiceResult<()> {
        let provider = payment.provider.as_str();
        let Some(record) = tx
            .saved_methods()
            .active(payment.client_id, provider)
            .await?
        else {
            return Ok(());
        };
        if record.created_at > payment.created_at {
            return Ok(());
        }
        tx.saved_methods().revoke(record.id).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "payment.saved_method_revoked".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "client_id": payment.client_id,
                    "provider": provider,
                    "title": record.title,
                }),
                ip: None,
            })
            .await?;
        tracing::info!(client = %payment.client_id, provider, payment = %payment.id, "клиент отозвал разрешение на автосписание, сохранённая карта снята");
        Ok(())
    }

    async fn accept_paid(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
        received: Option<Amount>,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate(&mut tx, provider, reference).await?;
        if let Some(txid) = repeat_txid(&payment, outcome.provider_txid.as_deref()) {
            if !self.repeat_confirmed(provider, outcome).await? {
                tracing::warn!(payment = %payment.id, provider, "платёжка не подтвердила повторную оплату");
                return Err(ServiceError::rejected(
                    REPEAT_UNCONFIRMED,
                    "платёжная система не подтвердила повторную оплату",
                ));
            }
            let result = self
                .accept_repeat(&mut tx, &payment, txid, outcome.amount)
                .await?;
            tx.commit().await?;
            return Ok(result);
        }
        let check = check_paid_amount(payment.amount, outcome.amount);
        if !check.accepts() {
            tracing::warn!(payment = %payment.id, ?check, "сумма оплаты не сошлась");
            return Err(ServiceError::rejected(
                "payment.amount_mismatch",
                match check {
                    AmountCheck::Missing => "провайдер не подтвердил сумму".to_string(),
                    other => format!("сумма не сошлась: {other:?}"),
                },
            ));
        }
        let result = self
            .settle_paid(
                &mut tx,
                payment,
                outcome.provider_txid.as_deref(),
                outcome.saved_method.as_ref(),
            )
            .await?;
        if let (
            Some(received),
            AcceptResult::Accepted {
                order_id,
                payment_id,
            },
        ) = (received, &result)
        {
            let paid = tx.payments().get(*payment_id).await?;
            let surplus = received.minor - paid.amount.minor;
            if received.currency == paid.amount.currency && surplus > 0 {
                let order = tx.orders().get(*order_id).await?;
                self.credit_received(
                    &mut tx,
                    provider,
                    &order,
                    &paid,
                    Amount::new(surplus, received.currency),
                    Received::Overpaid,
                )
                .await?;
            }
        }
        tx.commit().await?;
        Ok(result)
    }

    async fn repeat_confirmed(
        &self,
        provider: &str,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<bool> {
        let registry = self.services.payments();
        let Some(found) = registry.get(provider) else {
            return Ok(true);
        };
        Ok(found
            .confirm_repeat(&registry.config(provider), outcome)
            .await?)
    }

    async fn accept_repeat(
        &self,
        tx: &mut Tx,
        original: &Payment,
        txid: &str,
        received: Option<Amount>,
    ) -> ServiceResult<AcceptResult> {
        if let Some(existing) = tx
            .payments()
            .by_provider_txid(original.provider.as_str(), txid)
            .await?
        {
            if existing.order_id != original.order_id {
                return Err(ServiceError::rejected(
                    "payment.txid_mismatch",
                    "транзакция провайдера уже привязана к другому заказу",
                ));
            }
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: existing.id,
            });
        }
        let amount = received.filter(|a| a.minor > 0).ok_or_else(|| {
            ServiceError::rejected(
                "payment.amount_mismatch",
                "провайдер не подтвердил сумму повторной оплаты",
            )
        })?;
        let created = tx
            .payments()
            .create(&NewPayment {
                order_id: original.order_id,
                client_id: original.client_id,
                provider: original.provider.clone(),
                method: Some(REPEAT_METHOD.into()),
                amount,
                expires_at: None,
            })
            .await?;
        let paid = tx
            .payments()
            .transition(created.id, PaymentStatus::Paid, Some(txid), None)
            .await?
            .payment()
            .clone();
        let order = tx.orders().lock(original.order_id).await?;
        tracing::warn!(payment = %paid.id, original = %original.id, order = %order.id, "повторная оплата уже оплаченного счёта");
        self.repeat_payment(tx, &order, &paid, Some(original.id))
            .await?;
        tx.events()
            .push(&DomainEvent::PaymentPaid {
                payment_id: paid.id,
                order_id: order.id,
                client_id: paid.client_id,
                order_kind: order.kind,
                amount: paid.amount,
                provider: paid.provider.clone(),
            })
            .await?;
        Ok(AcceptResult::Accepted {
            order_id: order.id,
            payment_id: paid.id,
        })
    }

    async fn settle_paid(
        &self,
        tx: &mut Tx,
        payment: Payment,
        txid: Option<&str>,
        saved: Option<&SavedMethod>,
    ) -> ServiceResult<AcceptResult> {
        let change = match tx
            .payments()
            .transition(payment.id, PaymentStatus::Paid, txid, None)
            .await
        {
            Ok(change) => change,
            Err(StoreError::Invalid(_)) => {
                return Ok(AcceptResult::Ignored {
                    reason: format!("платёж уже {}", payment.status.as_str()),
                });
            }
            Err(StoreError::Conflict { message, .. }) => {
                return Err(ServiceError::rejected("payment.txid_mismatch", message));
            }
            Err(err) => return Err(err.into()),
        };
        let StatusChange::Applied(paid) = change else {
            if let Some(method) = saved.filter(|m| !m.token.trim().is_empty())
                && !tx
                    .saved_methods()
                    .kept_since(
                        payment.client_id,
                        payment.provider.as_str(),
                        payment.paid_at.unwrap_or_else(|| self.services.now()),
                    )
                    .await?
            {
                self.save_method(tx, &payment, method).await?;
            }
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        };
        if let Some(method) = saved {
            self.save_method(tx, &paid, method).await?;
        }
        let order = tx.orders().lock(paid.order_id).await?;
        crate::renewals::activate(self.services, tx, &paid, &order, self.services.now()).await?;
        let resumes =
            order.status == OrderStatus::Review && !paid_elsewhere(tx, order.id, paid.id).await?;
        let order =
            if matches!(order.status, OrderStatus::Pending | OrderStatus::Canceled) || resumes {
                let (order, _) = tx.orders().transition(order.id, OrderStatus::Paid).await?;
                if let Some(reason) = self.modules_reject(tx, &order, &paid).await? {
                    self.hold_for_review(tx, &order, &paid, &reason).await?;
                    order
                } else if let Some(reason) = self.paid_hold(tx, &paid).await? {
                    self.review_paid(tx, &order, &paid, &reason).await?;
                    order
                } else if order.kind == OrderKind::TopUp {
                    credit_topup(tx, &order, &paid).await?;
                    let (fulfilled, changed) = tx
                        .orders()
                        .transition(order.id, OrderStatus::Fulfilled)
                        .await?;
                    if changed {
                        tx.events()
                            .push(&DomainEvent::OrderFulfilled {
                                order_id: fulfilled.id,
                                client_id: fulfilled.client_id,
                                order_kind: fulfilled.kind,
                                subscription_id: None,
                            })
                            .await?;
                    }
                    fulfilled
                } else {
                    tx.jobs()
                        .enqueue(&JobPayload::FulfillOrder { order_id: order.id }, None)
                        .await?;
                    order
                }
            } else {
                let original = tx
                    .payments()
                    .for_order(order.id)
                    .await?
                    .into_iter()
                    .find(|p| p.id != paid.id && p.status.is_paid_like())
                    .map(|p| p.id);
                self.repeat_payment(tx, &order, &paid, original).await?;
                order
            };
        tx.events()
            .push(&DomainEvent::PaymentPaid {
                payment_id: paid.id,
                order_id: order.id,
                client_id: paid.client_id,
                order_kind: order.kind,
                amount: paid.amount,
                provider: paid.provider.clone(),
            })
            .await?;
        Ok(AcceptResult::Accepted {
            order_id: order.id,
            payment_id: paid.id,
        })
    }

    async fn modules_reject(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
    ) -> ServiceResult<Option<String>> {
        let ctx = self.services.module_context();
        match self
            .services
            .modules()
            .order_paid(&ctx, tx, order, paid)
            .await
        {
            Ok(()) => Ok(None),
            Err(ModuleError::Invalid(reason)) => Ok(Some(reason)),
            Err(err) => Err(err.into()),
        }
    }

    async fn paid_hold(&self, tx: &mut Tx, paid: &Payment) -> ServiceResult<Option<String>> {
        let underpaid = format!("{}:payment:{}", Received::Underpaid.key(), paid.id);
        if let Some(credited) = tx.ledger().credited_by_key(&underpaid).await? {
            return Ok(Some(format!(
                "оплата пришла полностью после недоплаты, а полученные раньше {} уже зачислены клиенту на баланс. \
                 Спишите их с баланса и выдайте заказ или верните оплату",
                amount_text(credited.into())
            )));
        }
        let reversed = tx
            .payment_notices()
            .for_payment(paid.id)
            .await?
            .iter()
            .any(|notice| {
                matches!(
                    notice.kind,
                    NoticeKind::RefundReported | NoticeKind::Chargeback
                )
            });
        Ok(reversed.then(|| {
            "платёжная система раньше сообщала о возврате по этому платежу. \
             Сверьте платёж в её кабинете, прежде чем выдавать заказ"
                .to_string()
        }))
    }

    async fn review_paid(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
        reason: &str,
    ) -> ServiceResult<()> {
        tracing::warn!(order = %order.id, payment = %paid.id, %reason, "оплаченный заказ ждёт ручной проверки");
        tx.orders().mark_review(order.id, reason).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "order.review".into(),
                entity_kind: Some(audit_entity::ORDER.into()),
                entity_id: Some(order.id.0),
                details: serde_json::json!({
                    "payment_id": paid.id,
                    "amount": paid.amount,
                    "reason": reason,
                }),
                ip: None,
            })
            .await?;
        self.notify(
            tx,
            paid.client_id,
            PAYMENT_REVIEW_TEMPLATE,
            (
                format!(
                    "Оплата {} получена. Заказ ждёт ручной проверки, мы свяжемся с вами.",
                    amount_text(paid.amount)
                ),
                format!(
                    "Payment of {} received. Your order is waiting for a manual review, we'll get in touch.",
                    amount_en(paid.amount)
                ),
            ),
            paid,
            order,
        )
        .await
    }

    async fn hold_for_review(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
        reason: &str,
    ) -> ServiceResult<()> {
        let reason = format!("оплата получена, но модуль не подтвердил заказ: {reason}");
        tracing::warn!(order = %order.id, payment = %paid.id, %reason, "оплаченный заказ ждёт ручной проверки");
        tx.orders().mark_review(order.id, &reason).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "order.review".into(),
                entity_kind: Some(audit_entity::ORDER.into()),
                entity_id: Some(order.id.0),
                details: serde_json::json!({
                    "payment_id": paid.id,
                    "amount": paid.amount,
                    "reason": reason,
                }),
                ip: None,
            })
            .await?;
        Ok(())
    }

    async fn repeat_payment(
        &self,
        tx: &mut Tx,
        order: &Order,
        paid: &Payment,
        original: Option<PaymentId>,
    ) -> ServiceResult<()> {
        let currency = self.services.settings().currency;
        let credit = paid
            .amount
            .as_money()
            .filter(|m| m.currency == currency && m.minor > 0);
        let action = if credit.is_some() {
            RepeatAction::Balance
        } else {
            RepeatAction::Review
        };
        if !tx.payments().mark_repeat(paid.id, original, action).await? {
            return Ok(());
        }
        let shown = amount_text(paid.amount);
        let shown_en = amount_en(paid.amount);
        let (audit_action, template, text) = match credit {
            Some(amount) => {
                let appended = tx
                    .ledger()
                    .append(&NewLedgerEntry {
                        client_id: paid.client_id,
                        wallet: Wallet::Main,
                        amount,
                        kind: LedgerKind::TopUp,
                        order_id: Some(order.id),
                        payment_id: Some(paid.id),
                        admin_id: None,
                        comment: Some(format!(
                            "Повторная оплата заказа {}: зачислено на баланс",
                            order.public_id
                        )),
                        idempotency_key: Some(format!("repeat:payment:{}", paid.id)),
                    })
                    .await?;
                if !appended.duplicate {
                    tx.events()
                        .push(&DomainEvent::BalanceChanged {
                            client_id: paid.client_id,
                            wallet: Wallet::Main,
                            kind: LedgerKind::TopUp,
                            delta: amount,
                        })
                        .await?;
                }
                (
                    "payment.repeat_credited",
                    REPEAT_CREDITED_TEMPLATE,
                    (
                        format!(
                            "Повторная оплата {shown} по уже оплаченному счёту зачислена на баланс."
                        ),
                        format!(
                            "A repeat payment of {shown_en} for an already paid invoice is on your balance."
                        ),
                    ),
                )
            }
            None => {
                tracing::warn!(payment = %paid.id, order = %order.id, "повторная оплата ждёт ручной проверки");
                (
                    "payment.repeat_review",
                    REPEAT_REVIEW_TEMPLATE,
                    (
                        format!(
                            "Повторная оплата {shown} по уже оплаченному счёту получена. Проверим её вручную и свяжемся с вами."
                        ),
                        format!(
                            "A repeat payment of {shown_en} for an already paid invoice arrived. We'll check it by hand and get in touch."
                        ),
                    ),
                )
            }
        };
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: audit_action.into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(paid.id.0),
                details: serde_json::json!({
                    "order_id": order.id,
                    "order_status": order.status.as_str(),
                    "amount": paid.amount,
                    "provider_txid": paid.provider_txid,
                    "original_payment_id": original,
                }),
                ip: None,
            })
            .await?;
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id: paid.client_id,
                    template: template.into(),
                    params: serde_json::json!({
                        "text": text.0,
                        "text_en": text.1,
                        "amount": paid.amount,
                        "payment_id": paid.id,
                        "order_id": order.id,
                    }),
                },
                None,
            )
            .await?;
        Ok(())
    }

    async fn save_method(
        &self,
        tx: &mut Tx,
        paid: &Payment,
        method: &SavedMethod,
    ) -> ServiceResult<()> {
        let Some(secrets) = self.services.secrets() else {
            tracing::warn!(payment = %paid.id, "карта не сохранена: не задан SN_SECRET_KEY");
            return Ok(());
        };
        let provider = paid.provider.as_str();
        let sealed = secrets.seal_str(
            &saved_method_context(paid.client_id, provider),
            &method.token,
        )?;
        tx.saved_methods()
            .save(paid.client_id, provider, &sealed, method.title.as_deref())
            .await?;
        Ok(())
    }

    async fn accept_failure(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
        target: PaymentStatus,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate(&mut tx, provider, reference).await?;
        if outcome.method_revoked() {
            self.revoke_method(&mut tx, &payment).await?;
        }
        let change = match tx
            .payments()
            .transition(
                payment.id,
                target,
                outcome.provider_txid.as_deref(),
                Some("по уведомлению провайдера"),
            )
            .await
        {
            Ok(change) => change,
            Err(StoreError::Invalid(_)) | Err(StoreError::Conflict { .. }) => {
                tx.commit().await?;
                return Ok(AcceptResult::Ignored {
                    reason: format!("платёж уже {}", payment.status.as_str()),
                });
            }
            Err(err) => return Err(err.into()),
        };
        if change.applied() {
            let order = tx.orders().lock(payment.order_id).await?;
            let held = order.status == OrderStatus::Review
                && !paid_elsewhere(&mut tx, order.id, payment.id).await?;
            if order.status == OrderStatus::Pending || held {
                let (canceled, changed) = tx
                    .orders()
                    .transition(order.id, OrderStatus::Canceled)
                    .await?;
                if changed {
                    let ctx = self.services.module_context();
                    self.services
                        .modules()
                        .order_canceled(&ctx, &mut tx, &canceled)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(AcceptResult::StatusChanged {
            payment_id: change.payment().id,
            status: change.payment().status,
        })
    }

    async fn accept_refund(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
        partial: Option<Amount>,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate_refunded(
            &mut tx,
            provider,
            reference,
            outcome.provider_txid.as_deref(),
        )
        .await?;
        if !payment.status.is_paid_like() {
            self.note_unpaid_refund(&mut tx, provider, &payment, outcome, partial)
                .await?;
            tx.commit().await?;
            return Ok(AcceptResult::Ignored {
                reason: format!("возврат по платежу в статусе {}", payment.status.as_str()),
            });
        }
        let refunds = tx.refunds().for_payment(payment.id).await?;
        if refunds.iter().any(|r| {
            r.provider_refund_id
                .as_deref()
                .is_some_and(|stored| same_refund(stored, &outcome.event_key))
        }) {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let already: i64 = refunds.iter().map(|r| r.amount.minor).sum();
        let remaining = payment.amount.minor - already;
        let requests = tx.payments().provider_refunds(payment.id).await?;
        if let Some(request) =
            matching_request(&requests, &outcome.event_key, partial.or(outcome.amount))
        {
            return self
                .confirm_request(tx, &payment, request, &outcome.event_key, remaining)
                .await;
        }
        if let Some(outside) = partial.or(outcome.amount).and_then(|reported| {
            refunds.iter().find(|r| {
                r.amount == reported
                    && r.provider_refund_id
                        .as_deref()
                        .is_some_and(|id| id.starts_with(OUTSIDE_REFUND_PREFIX))
            })
        }) {
            tx.refunds().relink(outside.id, &outcome.event_key).await?;
            tx.commit().await?;
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let delta = match partial {
            None => remaining,
            Some(amount) if amount.currency == payment.amount.currency => {
                amount.minor.min(remaining)
            }
            Some(_) => {
                return Err(ServiceError::rejected(
                    "payment.refund_currency",
                    "валюта возврата не совпала с валютой платежа",
                ));
            }
        };
        if delta <= 0 {
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let recorded = self
            .record_refund(
                &mut tx,
                &payment,
                &RefundSpec {
                    amount: Amount::new(delta, payment.amount.currency),
                    method: RefundMethod::Provider,
                    action: RefundSubscriptionAction::Keep,
                    reason: Some("возврат по уведомлению платёжной системы"),
                    admin: None,
                    provider_refund_id: Some(&outcome.event_key),
                    held_bonus: None,
                    request: None,
                },
            )
            .await?;
        tx.commit().await?;
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: recorded.status,
        })
    }

    async fn note_unpaid_refund(
        &self,
        tx: &mut Tx,
        provider: &str,
        payment: &Payment,
        outcome: &WebhookOutcome,
        partial: Option<Amount>,
    ) -> ServiceResult<()> {
        let reported = partial.or(outcome.amount).unwrap_or(payment.amount);
        let underpaid = format!("{}:payment:{}", Received::Underpaid.key(), payment.id);
        let credited = tx.ledger().credited_by_key(&underpaid).await?;
        let mut text = format!(
            "Платёжная система сообщила о возврате {} по платежу, который не засчитан как оплаченный. \
             Если она потом пришлёт оплату по этому счёту, заказ не выдастся сам и будет ждать проверки. \
             Сверьте платёж в кабинете платёжной системы",
            amount_text(reported)
        );
        if let Some(credited) = credited {
            text.push_str(&format!(
                ". Из этой оплаты клиенту уже зачислено на баланс {}: если деньги вернулись покупателю, спишите их с баланса",
                amount_text(credited.into())
            ));
        }
        let recorded = tx
            .payment_notices()
            .record(&NewPaymentNotice {
                provider: provider.to_string(),
                event_key: outcome.event_key.clone(),
                kind: NoticeKind::RefundReported,
                payment_id: Some(payment.id),
                client_id: Some(payment.client_id),
                amount: Some(reported),
                text: clip(&text, NOTICE_TEXT_LIMIT),
            })
            .await?;
        if recorded.is_some() {
            tracing::warn!(provider, payment = %payment.id, status = payment.status.as_str(), "платёжная система сообщила о возврате по неоплаченному платежу");
        }
        Ok(())
    }

    async fn confirm_request(
        &self,
        mut tx: Tx,
        payment: &Payment,
        request: &ProviderRefund,
        event_key: &str,
        remaining: i64,
    ) -> ServiceResult<AcceptResult> {
        let now = self.services.now();
        let minor = request.amount.minor.min(remaining);
        if minor <= 0 {
            tx.payments()
                .resolve_provider_refund(
                    request.id,
                    ProviderRefundStatus::Succeeded,
                    None,
                    None,
                    now,
                )
                .await?;
            self.release_hold(
                &mut tx,
                payment,
                request,
                request.amount.minor,
                request.bonus_minor,
            )
            .await?;
            tx.commit().await?;
            return Ok(AcceptResult::AlreadyApplied {
                payment_id: payment.id,
            });
        }
        let refund_key = match request.provider_refund_id.as_deref() {
            Some(id) if same_refund(id, event_key) => id.to_string(),
            _ => event_key.to_string(),
        };
        let held_bonus = match request.hold_entry_id {
            None => None,
            Some(_) if minor == request.amount.minor => Some(request.bonus_minor),
            Some(_) => Some(
                bonus::due(&mut tx, payment, minor, Some(request.id))
                    .await?
                    .min(request.bonus_minor),
            ),
        };
        let recorded = self
            .record_refund(
                &mut tx,
                payment,
                &RefundSpec {
                    amount: Amount::new(minor, payment.amount.currency),
                    method: RefundMethod::Provider,
                    action: request.subscription_action,
                    reason: request.reason.as_deref(),
                    admin: request.admin_id,
                    provider_refund_id: Some(&refund_key),
                    held_bonus,
                    request: Some(request.id),
                },
            )
            .await?;
        tx.payments()
            .resolve_provider_refund(
                request.id,
                ProviderRefundStatus::Succeeded,
                Some(recorded.refund_id),
                None,
                now,
            )
            .await?;
        self.release_hold(
            &mut tx,
            payment,
            request,
            request.amount.minor - minor,
            request.bonus_minor - recorded.bonus_minor,
        )
        .await?;
        let acts = request.subscription_action != RefundSubscriptionAction::Keep
            && recorded.order.subscription_id.is_some()
            && tx.payments().arm_refund_action(request.id, now).await?;
        if blocked_refund::requested_by_system(request) {
            blocked_refund::notify_client(
                &mut tx,
                &recorded.order,
                payment,
                Amount::new(minor, payment.amount.currency),
            )
            .await?;
        }
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "payment.refund_confirmed".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "request_id": request.id,
                    "refund_id": recorded.refund_id,
                    "amount": Amount::new(minor, payment.amount.currency),
                    "bonus": bonus::shown(recorded.bonus_minor, payment.amount),
                    "provider_refund_id": refund_key,
                    "requested_by": request.admin_id,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        if acts && let Err(err) = self.run_refund_action(request.id).await {
            tracing::warn!(payment = %payment.id, request = request.id, error = %err, "действие с подпиской после подтверждения возврата отложено до повтора");
        }
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: recorded.status,
        })
    }

    async fn decline_refund(
        &self,
        provider: &str,
        reference: &PaymentRef,
        outcome: &WebhookOutcome,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = locate_refunded(
            &mut tx,
            provider,
            reference,
            outcome.provider_txid.as_deref(),
        )
        .await?;
        let requests = tx.payments().provider_refunds(payment.id).await?;
        let Some(request) = matching_request(&requests, &outcome.event_key, outcome.amount) else {
            return Ok(AcceptResult::Ignored {
                reason: "отказ по возврату, которого нет среди ожидающих".into(),
            });
        };
        let reason = "платёжная система отклонила возврат";
        tx.payments()
            .resolve_provider_refund(
                request.id,
                ProviderRefundStatus::Declined,
                None,
                Some(reason),
                self.services.now(),
            )
            .await?;
        self.release_hold(
            &mut tx,
            &payment,
            request,
            request.amount.minor,
            request.bonus_minor,
        )
        .await?;
        self.resume_delivery(&mut tx, payment.order_id).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: "payment.refund_declined".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "request_id": request.id,
                    "amount": request.amount,
                    "provider_refund_id": request.provider_refund_id,
                    "event_key": outcome.event_key,
                    "requested_by": request.admin_id,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        tracing::warn!(payment = %payment.id, request = request.id, "платёжная система отклонила возврат");
        Ok(AcceptResult::StatusChanged {
            payment_id: payment.id,
            status: payment.status,
        })
    }

    async fn resume_delivery(&self, tx: &mut Tx, order_id: OrderId) -> ServiceResult<()> {
        let order = tx.orders().get(order_id).await?;
        if order.status == OrderStatus::Paid && !tx.orders().refund_pending(order.id).await? {
            let queued = tx
                .jobs()
                .ensure_queued(&JobPayload::FulfillOrder { order_id: order.id })
                .await?;
            if queued.armed() {
                tracing::info!(order = %order.id, job = %queued.id(), "возврат не состоялся: выдача заказа поставлена в очередь");
            }
        }
        Ok(())
    }

    async fn record_refund(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        spec: &RefundSpec<'_>,
    ) -> ServiceResult<RecordedRefund> {
        let already = refunded_minor(tx, payment.id).await?;
        let total = already
            .checked_add(spec.amount.minor)
            .ok_or(sn_domain::DomainError::AmountOverflow)?;
        let status = if total >= payment.amount.minor {
            PaymentStatus::Refunded
        } else {
            PaymentStatus::PartiallyRefunded
        };
        tx.payments()
            .transition(payment.id, status, None, None)
            .await?;
        let order = tx.orders().lock(payment.order_id).await?;
        let repeat = tx.payments().repeat_action(payment.id).await?;
        let reverses = spec.method == RefundMethod::Provider && credits_balance(&order, repeat);
        let bonus_minor = match spec.held_bonus {
            Some(held) => held,
            None if reverses => bonus::due(tx, payment, spec.amount.minor, spec.request).await?,
            None => 0,
        };
        let refund_id = tx
            .refunds()
            .create(&NewRefund {
                payment_id: payment.id,
                amount: spec.amount,
                method: spec.method,
                subscription_action: spec.action,
                reason: spec.reason.map(str::to_string),
                admin_id: spec.admin,
                provider_refund_id: spec.provider_refund_id.map(str::to_string),
                bonus_minor,
            })
            .await?;
        match spec.method {
            RefundMethod::Balance => {
                let amount = spec.amount.as_money().ok_or_else(|| {
                    ServiceError::rejected(
                        "refund.balance_currency",
                        "звёзды на баланс не возвращаются",
                    )
                })?;
                let appended = tx
                    .ledger()
                    .append(&NewLedgerEntry {
                        client_id: payment.client_id,
                        wallet: Wallet::Main,
                        amount,
                        kind: LedgerKind::Refund,
                        order_id: Some(order.id),
                        payment_id: Some(payment.id),
                        admin_id: None,
                        comment: spec.reason.map(str::to_string),
                        idempotency_key: Some(format!("refund:{refund_id}")),
                    })
                    .await?;
                if !appended.duplicate {
                    tx.events()
                        .push(&DomainEvent::BalanceChanged {
                            client_id: payment.client_id,
                            wallet: Wallet::Main,
                            kind: LedgerKind::Refund,
                            delta: amount,
                        })
                        .await?;
                }
            }
            RefundMethod::Provider if reverses && spec.held_bonus.is_none() => {
                self.reverse_topup(tx, payment, &order, spec.amount, bonus_minor, refund_id)
                    .await?;
            }
            RefundMethod::Provider => {}
        }
        let order = if status == PaymentStatus::Refunded
            && repeat.is_none()
            && matches!(
                order.status,
                OrderStatus::Paid | OrderStatus::Review | OrderStatus::Fulfilled
            ) {
            tx.orders()
                .transition(order.id, OrderStatus::Refunded)
                .await?
                .0
        } else {
            order
        };
        tx.events()
            .push(&DomainEvent::PaymentRefunded {
                payment_id: payment.id,
                order_id: order.id,
                client_id: payment.client_id,
                amount: spec.amount,
            })
            .await?;
        Ok(RecordedRefund {
            refund_id,
            status,
            order,
            bonus_minor,
        })
    }

    async fn reverse_topup(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        order: &Order,
        amount: Amount,
        bonus_minor: i64,
        refund_id: RefundId,
    ) -> ServiceResult<()> {
        let Some(refunded) = amount.as_money() else {
            return Ok(());
        };
        let balance = tx
            .ledger()
            .balance(payment.client_id, Wallet::Main, refunded.currency)
            .await?;
        let debit = Money::new(balance.minor.min(refunded.minor), refunded.currency);
        if debit.minor > 0 {
            let delta = debit.checked_neg()?;
            let appended = tx
                .ledger()
                .append(&NewLedgerEntry {
                    client_id: payment.client_id,
                    wallet: Wallet::Main,
                    amount: delta,
                    kind: LedgerKind::Purchase,
                    order_id: Some(order.id),
                    payment_id: Some(payment.id),
                    admin_id: None,
                    comment: Some(format!(
                        "Сторно пополнения: возврат платежа {}",
                        payment.public_id
                    )),
                    idempotency_key: Some(format!("topup_reversal:{refund_id}")),
                })
                .await?;
            if !appended.duplicate {
                tx.events()
                    .push(&DomainEvent::BalanceChanged {
                        client_id: payment.client_id,
                        wallet: Wallet::Main,
                        kind: LedgerKind::Purchase,
                        delta,
                    })
                    .await?;
            }
        }
        let bonus = Money::new(bonus_minor, refunded.currency);
        let bonus_debit = Money::new(
            (balance.minor - debit.minor).min(bonus.minor).max(0),
            refunded.currency,
        );
        bonus::take(
            tx,
            payment,
            order.id,
            bonus_debit,
            format!("topup_bonus_reversal:{refund_id}"),
        )
        .await?;
        let short = Money::new(
            refunded.minor - debit.minor + bonus.minor - bonus_debit.minor,
            refunded.currency,
        );
        if short.minor > 0 {
            tracing::warn!(payment = %payment.id, "возврат пополнения больше остатка на балансе");
            let taken = debit.checked_add(bonus_debit)?;
            let wanted = if bonus.is_zero() {
                String::new()
            } else {
                format!(
                    " (вместе с возвратом снимается и бонус за пополнение {})",
                    format::money_exact(bonus)
                )
            };
            tx.payment_notices()
                .record(&NewPaymentNotice {
                    provider: payment.provider.as_str().to_string(),
                    event_key: format!("refund_shortfall:{refund_id}"),
                    kind: NoticeKind::RefundReported,
                    payment_id: Some(payment.id),
                    client_id: Some(payment.client_id),
                    amount: Some(short.into()),
                    text: format!(
                        "Платёжная система вернула покупателю {}, но клиент уже потратил часть \
                         пополнения: с баланса списано {}, не хватило {}{wanted}. Решите вручную",
                        format::money(refunded),
                        format::money_exact(taken),
                        format::money_exact(short)
                    ),
                })
                .await?;
            tx.audit()
                .record(&AuditEntry {
                    actor_kind: ActorKind::System,
                    actor_id: None,
                    action: "refund.topup_shortfall".into(),
                    entity_kind: Some("payment".into()),
                    entity_id: Some(payment.id.0),
                    details: serde_json::json!({
                        "refund_id": refund_id,
                        "refunded": refunded,
                        "debited": debit,
                        "bonus": bonus,
                        "bonus_debited": bonus_debit,
                    }),
                    ip: None,
                })
                .await?;
        }
        Ok(())
    }

    async fn hold_refund(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        order: &Order,
        money: Money,
        admin: Option<AdminId>,
    ) -> ServiceResult<bonus::Hold> {
        let bonus = Money::new(
            bonus::due(tx, payment, money.minor, None).await?,
            money.currency,
        );
        let needed = money.checked_add(bonus)?;
        let spent = || {
            ServiceError::rejected(
                "refund.topup_spent",
                if bonus.is_zero() {
                    "Клиент уже потратил пополнение: на балансе меньше суммы возврата".to_string()
                } else {
                    format!(
                        "Клиент уже потратил пополнение: вместе с возвратом снимается бонус {}, на балансе нужно {}",
                        format::money_exact(bonus),
                        format::money_exact(needed)
                    )
                },
            )
        };
        let unheld = tx
            .payments()
            .unheld_refunds_minor(payment.client_id, money.currency.into())
            .await?;
        let balance = tx
            .ledger()
            .balance(payment.client_id, Wallet::Main, money.currency)
            .await?;
        if balance.minor - unheld < needed.minor {
            return Err(spent());
        }
        let delta = money.checked_neg()?;
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: payment.client_id,
                wallet: Wallet::Main,
                amount: delta,
                kind: LedgerKind::RefundHold,
                order_id: Some(order.id),
                payment_id: Some(payment.id),
                admin_id: admin,
                comment: Some(format!(
                    "Возврат пополнения через платёжную систему: платёж {}",
                    payment.public_id
                )),
                idempotency_key: None,
            })
            .await
            .map_err(|err| match err {
                StoreError::InsufficientFunds => spent(),
                other => other.into(),
            })?;
        tx.events()
            .push(&DomainEvent::BalanceChanged {
                client_id: payment.client_id,
                wallet: Wallet::Main,
                kind: LedgerKind::RefundHold,
                delta,
            })
            .await?;
        bonus::take(
            tx,
            payment,
            order.id,
            bonus,
            format!("topup_bonus_hold:{}", appended.entry_id),
        )
        .await
        .map_err(|err| match err {
            ServiceError::Store(StoreError::InsufficientFunds)
            | ServiceError::InsufficientFunds => spent(),
            other => other,
        })?;
        Ok(bonus::Hold {
            entry: appended.entry_id,
            bonus_minor: bonus.minor,
        })
    }

    async fn release_hold(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        request: &ProviderRefund,
        minor: i64,
        bonus_minor: i64,
    ) -> ServiceResult<()> {
        if request.hold_entry_id.is_none() {
            return Ok(());
        }
        let Some(currency) = request.amount.currency.as_currency() else {
            return Ok(());
        };
        bonus::release(tx, payment, request.id, Money::new(bonus_minor, currency)).await?;
        if minor <= 0 {
            return Ok(());
        }
        let amount = Money::new(minor, currency);
        let appended = tx
            .ledger()
            .append(&NewLedgerEntry {
                client_id: payment.client_id,
                wallet: Wallet::Main,
                amount,
                kind: LedgerKind::RefundRelease,
                order_id: Some(payment.order_id),
                payment_id: Some(payment.id),
                admin_id: None,
                comment: Some(format!(
                    "Возврат платежа {} через платёжную систему не состоялся",
                    payment.public_id
                )),
                idempotency_key: Some(format!("refund_release:{}", request.id)),
            })
            .await?;
        if !appended.duplicate {
            tx.events()
                .push(&DomainEvent::BalanceChanged {
                    client_id: payment.client_id,
                    wallet: Wallet::Main,
                    kind: LedgerKind::RefundRelease,
                    delta: amount,
                })
                .await?;
        }
        Ok(())
    }

    pub async fn refund(
        &self,
        payment_id: PaymentId,
        command: &RefundCommand,
        admin: AdminId,
    ) -> ServiceResult<RefundResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = tx.payments().lock(payment_id).await?;
        if let Some(key) = command.key
            && let Some(done) = replay(&mut tx, &payment, key).await?
        {
            return Ok(done);
        }
        match self
            .refund_locked(&mut tx, &payment, command, Some(admin))
            .await?
        {
            RefundStep::Requested {
                request_id,
                amount,
                provider_refund_id,
                hold,
            } => {
                tx.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Admin,
                        actor_id: Some(admin.0),
                        action: REFUND_REQUESTED_AUDIT.into(),
                        entity_kind: Some("payment".into()),
                        entity_id: Some(payment.id.0),
                        details: serde_json::json!({
                            "request_id": request_id,
                            "amount": amount,
                            "method": command.method,
                            "subscription_action": command.subscription_action,
                            "provider_refund_id": provider_refund_id,
                            "hold_entry_id": hold,
                        }),
                        ip: None,
                    })
                    .await?;
                tx.commit().await?;
                Ok(RefundResult {
                    refund_id: None,
                    request_id: Some(request_id),
                    pending: true,
                    payment_id: payment.id,
                    amount,
                    status: payment.status,
                    subscription_error: None,
                })
            }
            RefundStep::Recorded { recorded, amount } => {
                tx.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Admin,
                        actor_id: Some(admin.0),
                        action: if command.outside {
                            "payment.refund_outside".into()
                        } else {
                            "payment.refund".into()
                        },
                        entity_kind: Some("payment".into()),
                        entity_id: Some(payment.id.0),
                        details: serde_json::json!({
                            "refund_id": recorded.refund_id,
                            "amount": amount,
                            "bonus": bonus::shown(recorded.bonus_minor, amount),
                            "method": command.method,
                            "subscription_action": command.subscription_action,
                        }),
                        ip: None,
                    })
                    .await?;
                tx.commit().await?;
                let subscription_error = match self
                    .refund_subscription_action(
                        &recorded.order,
                        command.subscription_action,
                        Some(admin),
                    )
                    .await
                {
                    Ok(()) => None,
                    Err(err) => {
                        tracing::warn!(payment = %payment.id, error = %err, "действие с подпиской после возврата не выполнено");
                        Some(err.to_string())
                    }
                };
                Ok(RefundResult {
                    refund_id: Some(recorded.refund_id),
                    request_id: None,
                    pending: false,
                    payment_id: payment.id,
                    amount,
                    status: recorded.status,
                    subscription_error,
                })
            }
        }
    }

    async fn refund_locked(
        &self,
        tx: &mut Tx,
        payment: &Payment,
        command: &RefundCommand,
        admin: Option<AdminId>,
    ) -> ServiceResult<RefundStep> {
        let currency = self.services.settings().currency;
        let now = self.services.now();
        let key = command.key.unwrap_or_else(Uuid::new_v4);
        if !matches!(
            payment.status,
            PaymentStatus::Paid | PaymentStatus::PartiallyRefunded
        ) {
            return Err(ServiceError::rejected(
                "refund.not_paid",
                "Вернуть можно только оплаченный платёж",
            ));
        }
        let already = refunded_minor(tx, payment.id).await?;
        let waiting = pending_minor(&tx.payments().provider_refunds(payment.id).await?);
        let remaining = payment.amount.minor - already - waiting;
        let minor = match command.amount {
            None => remaining,
            Some(amount) if amount.currency == payment.amount.currency => amount.minor,
            Some(_) => {
                return Err(ServiceError::rejected(
                    "refund.currency",
                    "Валюта возврата не совпадает с валютой платежа",
                ));
            }
        };
        if minor <= 0 || minor > remaining {
            if waiting > 0 {
                return Err(ServiceError::rejected(
                    "refund.pending",
                    "Возврат по этому платежу уже ждёт подтверждения платёжной системы",
                ));
            }
            return Err(ServiceError::rejected(
                "refund.amount",
                "Сумма возврата больше оплаченной или равна нулю",
            ));
        }
        let amount = Amount::new(minor, payment.amount.currency);
        let order = tx.orders().get(payment.order_id).await?;
        let credited = credits_balance(&order, tx.payments().repeat_action(payment.id).await?);
        let (provider_refund_id, completed, hold) = match command.method {
            RefundMethod::Balance if command.outside => {
                return Err(ServiceError::rejected(
                    "refund.outside_balance",
                    "Вне панели деньги возвращают через платёжную систему, а не на баланс",
                ));
            }
            RefundMethod::Provider if command.outside => {
                if matches!(
                    payment.provider.as_str(),
                    providers::BALANCE | providers::STARS
                ) {
                    return Err(ServiceError::rejected(
                        "refund.outside_provider",
                        "Эту оплату нельзя отметить возвращённой вне панели",
                    ));
                }
                (Some(format!("{OUTSIDE_REFUND_PREFIX}{key}")), true, None)
            }
            RefundMethod::Balance => {
                if credited {
                    return Err(ServiceError::rejected(
                        "refund.topup_to_balance",
                        "Пополнение возвращается только через платёжную систему",
                    ));
                }
                if amount.as_money().is_none_or(|m| m.currency != currency) {
                    return Err(ServiceError::rejected(
                        "refund.balance_currency",
                        "На баланс возвращаются только суммы в валюте установки",
                    ));
                }
                (None, true, None)
            }
            RefundMethod::Provider => {
                let provider = self
                    .services
                    .payments()
                    .get(payment.provider.as_str())
                    .filter(|p| p.kind() != ProviderKind::Balance && p.supports_refunds())
                    .ok_or_else(|| {
                        ServiceError::rejected(
                            "refund.provider_unsupported",
                            "Эта платёжная система не умеет возвращать деньги",
                        )
                    })?;
                let txid = payment.provider_txid.clone().ok_or_else(|| {
                    ServiceError::rejected(
                        "refund.no_txid",
                        "У платежа нет номера транзакции в платёжной системе",
                    )
                })?;
                let hold = match amount.as_money() {
                    Some(money) if credited => {
                        Some(self.hold_refund(tx, payment, &order, money, admin).await?)
                    }
                    _ => None,
                };
                let config = self.services.payments().config(provider.id());
                let email = tx
                    .clients()
                    .identities(payment.client_id)
                    .await?
                    .into_iter()
                    .find(|i| i.kind == sn_domain::client::IdentityKind::Email)
                    .map(|i| i.value);
                let description =
                    crate::checkout::describe(&self.services.settings().brand_name, &order.quote);
                let outcome = provider
                    .refund(
                        &config,
                        &RefundRequest {
                            payment_id: payment.id,
                            provider_txid: txid,
                            amount,
                            reason: command.reason.clone(),
                            key,
                            email,
                            description: Some(description),
                        },
                    )
                    .await?;
                (outcome.provider_refund_id, outcome.completed, hold)
            }
        };
        if !completed {
            let request = tx
                .payments()
                .create_provider_refund(&NewProviderRefund {
                    payment_id: payment.id,
                    amount,
                    subscription_action: command.subscription_action,
                    reason: command.reason.clone(),
                    admin_id: admin,
                    provider_refund_id: provider_refund_id.clone(),
                    hold_entry_id: hold.map(|h| h.entry),
                    bonus_minor: hold.map_or(0, |h| h.bonus_minor),
                    requested_at: now,
                })
                .await?;
            tx.payments()
                .set_provider_refund_key(request.id, key)
                .await?;
            tx.jobs()
                .enqueue(
                    &JobPayload::PollPayment {
                        payment_id: payment.id,
                    },
                    Some(now + Duration::seconds(REFUND_POLL_DELAY_SECONDS)),
                )
                .await?;
            return Ok(RefundStep::Requested {
                request_id: request.id,
                amount,
                provider_refund_id,
                hold: hold.map(|h| h.entry),
            });
        }
        let recorded = self
            .record_refund(
                tx,
                payment,
                &RefundSpec {
                    amount,
                    method: command.method,
                    action: command.subscription_action,
                    reason: command.reason.as_deref(),
                    admin,
                    provider_refund_id: provider_refund_id.as_deref(),
                    held_bonus: hold.map(|h| h.bonus_minor),
                    request: None,
                },
            )
            .await?;
        tx.refunds()
            .set_request_key(recorded.refund_id, key)
            .await?;
        Ok(RefundStep::Recorded {
            recorded: Box::new(recorded),
            amount,
        })
    }

    pub async fn refund_keyed(&self, payment_id: PaymentId, key: Uuid) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        if let Some((_, owner)) = conn.refunds().by_request_key(key).await? {
            return Ok(owner == payment_id);
        }
        Ok(conn
            .payments()
            .provider_refund_by_key(key)
            .await?
            .is_some_and(|r| r.payment_id == payment_id))
    }

    pub async fn refund_options(&self, payment_id: PaymentId) -> ServiceResult<RefundOptions> {
        let currency = self.services.settings().currency;
        let mut conn = self.services.store().conn().await?;
        let payment = conn.payments().get(payment_id).await?;
        let already: i64 = conn
            .refunds()
            .for_payment(payment.id)
            .await?
            .iter()
            .map(|r| r.amount.minor)
            .sum();
        let waiting = pending_minor(&conn.payments().provider_refunds(payment.id).await?);
        let remaining = (payment.amount.minor - already - waiting).max(0);
        let order = conn.orders().get(payment.order_id).await?;
        let grant = refund_action::gift_grant(&mut conn, order.id).await?;
        let topup = credits_balance(&order, conn.payments().repeat_action(payment.id).await?);
        let bonus = if topup {
            bonus::left(&mut conn, &payment).await?
        } else {
            None
        };
        let registry = self.services.payments();
        let provider = registry.get(payment.provider.as_str());
        let title = provider.as_ref().map_or_else(
            || payment.provider.as_str().to_string(),
            |p| p.title().to_string(),
        );
        let note = if !matches!(
            payment.status,
            PaymentStatus::Paid | PaymentStatus::PartiallyRefunded
        ) {
            Some("Вернуть можно только оплаченный платёж".to_string())
        } else if remaining <= 0 && waiting > 0 {
            Some("Возврат по этому платежу уже ждёт подтверждения платёжной системы".to_string())
        } else if remaining <= 0 {
            Some("Платёж уже возвращён полностью".to_string())
        } else {
            None
        };
        let to_balance = if topup {
            RefundWay::closed("Пополнение баланса возвращается только через платёжную систему")
        } else if payment
            .amount
            .as_money()
            .is_none_or(|m| m.currency != currency)
        {
            RefundWay::closed("На баланс возвращаются только суммы в валюте магазина")
        } else {
            RefundWay::open()
        };
        let internal = matches!(
            payment.provider.as_str(),
            providers::BALANCE | providers::STARS
        );
        let via_provider = match &provider {
            _ if payment.provider.as_str() == providers::BALANCE => {
                RefundWay::closed("Оплату с баланса возвращают на баланс")
            }
            Some(p) if p.kind() != ProviderKind::Balance && p.supports_refunds() => {
                if payment.provider_txid.is_some() {
                    RefundWay::open()
                } else {
                    RefundWay::closed("У платежа нет номера транзакции в платёжной системе")
                }
            }
            _ => RefundWay::closed(format!(
                "{title} не возвращает деньги через панель: верните их в кабинете {title} и отметьте возврат"
            )),
        };
        let outside = if internal {
            RefundWay::closed("Эту оплату нельзя отметить возвращённой вне панели")
        } else {
            RefundWay::open()
        };
        Ok(RefundOptions {
            payment_id: payment.id,
            status: payment.status,
            provider: payment.provider.as_str().to_string(),
            provider_title: title,
            amount: payment.amount,
            refunded: Amount::new(already, payment.amount.currency),
            waiting: Amount::new(waiting, payment.amount.currency),
            remaining: Amount::new(remaining, payment.amount.currency),
            topup,
            subscription: grant.as_ref().unwrap_or(&order).subscription_id.is_some(),
            days: grant.as_ref().unwrap_or(&order).quote.days,
            note,
            to_balance,
            via_provider,
            outside,
            bonus,
            gift: grant.as_ref().map(|grant| GiftRecipient {
                client_id: grant.client_id,
                subscription_id: grant.subscription_id,
            }),
        })
    }

    pub async fn mark_paid(
        &self,
        payment_id: PaymentId,
        admin: AdminId,
        note: Option<&str>,
    ) -> ServiceResult<AcceptResult> {
        let mut tx = self.services.store().begin().await?;
        let payment = tx.payments().lock(payment_id).await?;
        match payment.provider.as_str() {
            providers::STARS => {
                return Err(ServiceError::rejected(
                    "payment.manual_stars",
                    "Оплату звёздами подтверждает только Telegram",
                ));
            }
            providers::BALANCE => {
                return Err(ServiceError::rejected(
                    "payment.manual_balance",
                    "Оплата с баланса проводится без ручной отметки",
                ));
            }
            _ => {}
        }
        let underpaid = format!("{}:payment:{}", Received::Underpaid.key(), payment.id);
        if let Some(credited) = tx.ledger().credited_by_key(&underpaid).await? {
            return Err(ServiceError::rejected(
                "payment.underpaid_credited",
                format!(
                    "Полученные по этому счёту {} уже зачислены клиенту на баланс. Отмечать счёт оплаченным нельзя: клиент оплатит заказ с баланса",
                    amount_text(credited.into())
                ),
            ));
        }
        let result = self.settle_paid(&mut tx, payment, None, None).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "payment.mark_paid".into(),
                entity_kind: Some("payment".into()),
                entity_id: Some(payment_id.0),
                details: serde_json::json!({ "note": note }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn poll_payment(&self, payment_id: PaymentId) -> ServiceResult<PollResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(payment_id).await?
        };
        if payment.status.is_paid_like() {
            return self.poll_refunds_of(&payment).await;
        }
        if payment.status != PaymentStatus::Pending {
            return Ok(PollResult::Settled);
        }
        if payment
            .expires_at
            .is_some_and(|at| at < self.services.now())
        {
            return Ok(PollResult::Settled);
        }
        if self
            .services
            .payments()
            .get(payment.provider.as_str())
            .is_none_or(|p| p.kind() != ProviderKind::External)
        {
            return Ok(PollResult::Unsupported);
        }
        let Some(txid) = payment.provider_txid.clone() else {
            return self.confirm_by_payment(&payment).await;
        };
        self.confirm_by_txid(payment.id, &txid).await
    }

    async fn confirm_by_payment(&self, payment: &Payment) -> ServiceResult<PollResult> {
        let Some(provider) = self
            .services
            .payments()
            .get(payment.provider.as_str())
            .filter(|p| p.kind() == ProviderKind::External && p.polls_by_payment_id())
        else {
            return Ok(PollResult::Pending);
        };
        let config = self.services.payments().config(provider.id());
        let outcome = match provider
            .fetch_status(&config, &payment.public_id.to_string())
            .await
        {
            Ok(Some(outcome)) => outcome,
            Ok(None) => return Ok(PollResult::Pending),
            Err(PaymentError::NotImplemented(_))
            | Err(PaymentError::Unsupported(_))
            | Err(PaymentError::NotConfigured(_)) => return Ok(PollResult::Unsupported),
            Err(err) => return Err(err.into()),
        };
        let Some(txid) = outcome
            .provider_txid
            .clone()
            .filter(|t| !t.trim().is_empty())
        else {
            return Ok(PollResult::Pending);
        };
        if matches!(
            outcome.status,
            ProviderStatus::Pending | ProviderStatus::Failed | ProviderStatus::Canceled
        ) {
            return Ok(PollResult::Pending);
        }
        self.settle_polled(provider.as_ref(), payment.id, &txid, outcome)
            .await
    }

    async fn settle_polled(
        &self,
        provider: &dyn PaymentProvider,
        payment_id: PaymentId,
        txid: &str,
        outcome: WebhookOutcome,
    ) -> ServiceResult<PollResult> {
        let outcome = WebhookOutcome {
            payment: Some(PaymentRef::Payment(payment_id)),
            provider_txid: Some(txid.to_string()),
            ..outcome
        };
        self.apply_outcome(provider.id(), &outcome).await?;
        Ok(PollResult::Settled)
    }

    fn chain_providers(&self) -> Vec<&'static str> {
        self.services
            .payments()
            .providers()
            .iter()
            .filter(|p| p.polls_status() && p.kind() == ProviderKind::External)
            .map(|p| p.id())
            .collect()
    }

    pub async fn watch_chain(&self, now: DateTime<Utc>) -> ServiceResult<ChainWatchReport> {
        let providers = self.chain_providers();
        let mut report = ChainWatchReport::default();
        if providers.is_empty() {
            return Ok(report);
        }
        let payments = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .chain_watch(
                    &providers,
                    now - Duration::hours(CHAIN_LATE_HOURS),
                    CHAIN_WATCH_LIMIT,
                )
                .await?
        };
        for payment in payments {
            let Some(txid) = payment.provider_txid.as_deref() else {
                continue;
            };
            report.checked += 1;
            match self.confirm_by_txid(payment.id, txid).await {
                Ok(PollResult::Settled) => report.settled += 1,
                Ok(_) => {}
                Err(err) => {
                    report.failed += 1;
                    tracing::debug!(payment = %payment.id, error = %err, "перевод в блокчейне не проверен");
                }
            }
        }
        Ok(report)
    }

    pub async fn check_chain(&self, payment_id: PaymentId) -> ServiceResult<PollResult> {
        let late_since = self.services.now() - Duration::hours(CHAIN_LATE_HOURS);
        let found = {
            let mut conn = self.services.store().conn().await?;
            let payment = conn.payments().get(payment_id).await?;
            if payment.status.is_paid_like() {
                return Ok(PollResult::Settled);
            }
            if !self.chain_providers().contains(&payment.provider.as_str()) {
                return Ok(PollResult::Unsupported);
            }
            conn.payments()
                .chain_candidate(payment_id, late_since)
                .await?
        };
        match found.as_ref().and_then(|p| p.provider_txid.as_deref()) {
            Some(txid) => self.confirm_by_txid(payment_id, txid).await,
            None => Ok(PollResult::Unsupported),
        }
    }

    pub async fn track_pending(
        &self,
        payment_id: PaymentId,
        provider_txid: &str,
    ) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        if conn
            .payments()
            .remember_txid(payment_id, provider_txid)
            .await?
        {
            return Ok(());
        }
        let payment = conn.payments().get(payment_id).await?;
        match payment.provider_txid.as_deref() {
            Some(known) if known == provider_txid => Ok(()),
            Some(known) => Err(ServiceError::rejected(
                "payment.txid_mismatch",
                format!("у платежа уже другая транзакция провайдера: {known}"),
            )),
            None => Err(ServiceError::rejected(
                "payment.not_pending",
                format!(
                    "номер транзакции запоминается только у неоплаченного платежа, сейчас {}",
                    payment.status.as_str()
                ),
            )),
        }
    }

    pub async fn confirm_by_txid(
        &self,
        payment_id: PaymentId,
        provider_txid: &str,
    ) -> ServiceResult<PollResult> {
        let payment = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().get(payment_id).await?
        };
        if payment.status.is_paid_like() {
            return Ok(PollResult::Settled);
        }
        if let Some(known) = payment.provider_txid.as_deref()
            && known != provider_txid
        {
            return Err(ServiceError::rejected(
                "payment.txid_mismatch",
                format!("у платежа уже другая транзакция провайдера: {known}"),
            ));
        }
        let Some(provider) = self
            .services
            .payments()
            .get(payment.provider.as_str())
            .filter(|p| p.kind() == ProviderKind::External)
        else {
            return Ok(PollResult::Unsupported);
        };
        let config = self.services.payments().config(provider.id());
        let external = if provider.status_by_order() {
            payment.public_id.to_string()
        } else {
            provider_txid.to_string()
        };
        let outcome = match provider.fetch_status(&config, &external).await {
            Ok(Some(outcome)) => outcome,
            Ok(None)
            | Err(PaymentError::NotImplemented(_))
            | Err(PaymentError::Unsupported(_))
            | Err(PaymentError::NotConfigured(_)) => return Ok(PollResult::Unsupported),
            Err(err) => return Err(err.into()),
        };
        if outcome.status == ProviderStatus::Pending {
            self.track_pending(payment.id, provider_txid).await?;
            return Ok(PollResult::Pending);
        }
        self.settle_polled(provider.as_ref(), payment.id, provider_txid, outcome)
            .await
    }

    async fn poll_refunds_of(&self, payment: &Payment) -> ServiceResult<PollResult> {
        let requests = {
            let mut conn = self.services.store().conn().await?;
            conn.payments().provider_refunds(payment.id).await?
        };
        let mut waiting = false;
        let mut polled = false;
        for request in pending_requests(&requests) {
            match self.poll_refund(payment, request).await? {
                RefundPoll::Settled => polled = true,
                RefundPoll::Waiting => {
                    polled = true;
                    waiting = true;
                }
                RefundPoll::Unsupported => {}
            }
        }
        Ok(if waiting {
            PollResult::Pending
        } else if polled || pending_requests(&requests).next().is_none() {
            PollResult::Settled
        } else {
            PollResult::Unsupported
        })
    }

    async fn poll_refund(
        &self,
        payment: &Payment,
        request: &ProviderRefund,
    ) -> ServiceResult<RefundPoll> {
        let Some(refund_id) = request.provider_refund_id.as_deref() else {
            return Ok(RefundPoll::Unsupported);
        };
        let Some(provider) = self.services.payments().get(payment.provider.as_str()) else {
            return Ok(RefundPoll::Unsupported);
        };
        let config = self.services.payments().config(provider.id());
        let outcome = match provider.fetch_refund(&config, refund_id).await {
            Ok(Some(outcome)) => outcome,
            Ok(None)
            | Err(PaymentError::NotImplemented(_))
            | Err(PaymentError::Unsupported(_))
            | Err(PaymentError::NotConfigured(_)) => return Ok(RefundPoll::Unsupported),
            Err(err) => return Err(err.into()),
        };
        match outcome.status {
            ProviderStatus::Refunded
            | ProviderStatus::PartiallyRefunded { .. }
            | ProviderStatus::RefundDeclined => {
                let outcome = WebhookOutcome {
                    payment: Some(PaymentRef::Payment(payment.id)),
                    provider_txid: payment.provider_txid.clone(),
                    ..outcome
                };
                self.apply_outcome(provider.id(), &outcome).await?;
                Ok(RefundPoll::Settled)
            }
            _ => Ok(RefundPoll::Waiting),
        }
    }

    pub async fn poll_refunds(&self, limit: i64) -> ServiceResult<RefundPollReport> {
        let now = self.services.now();
        let quiet_since = now - Duration::seconds(REFUND_POLL_DELAY_SECONDS);
        let pollable: Vec<String> = self
            .services
            .payments()
            .providers()
            .iter()
            .filter(|p| p.supports_refund_polling())
            .map(|p| p.id().to_string())
            .collect();
        let requests = {
            let mut conn = self.services.store().conn().await?;
            conn.payments()
                .claim_refunds_to_poll(&pollable, quiet_since, quiet_since, now, limit)
                .await?
        };
        let mut report = RefundPollReport::default();
        for request in &requests {
            let polled = match self.payment(request.payment_id).await {
                Ok(payment) => self.poll_refund(&payment, request).await,
                Err(err) => Err(err),
            };
            match polled {
                Ok(RefundPoll::Settled) => report.settled += 1,
                Ok(RefundPoll::Waiting) => report.waiting += 1,
                Ok(RefundPoll::Unsupported) => report.unsupported += 1,
                Err(err) => {
                    tracing::warn!(request = request.id, payment = %request.payment_id, error = %err, "статус возврата не получен");
                    report.failed += 1;
                }
            }
        }
        let actions = self.retry_refund_actions(limit).await?;
        report.actions_done = actions.done;
        report.actions_retry = actions.retry;
        report.actions_failed = actions.failed;
        Ok(report)
    }

    async fn payment(&self, payment_id: PaymentId) -> ServiceResult<Payment> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.payments().get(payment_id).await?)
    }

    pub async fn stars_pre_checkout(&self, query: &StarsPreCheckout) -> ServiceResult<()> {
        let public_id = Uuid::parse_str(&query.invoice_payload)
            .map_err(|_| ServiceError::rejected("stars.unknown_invoice", "счёт не найден"))?;
        let mut conn = self.services.store().conn().await?;
        let payment = conn.payments().by_public_id(public_id).await?;
        if payment.provider.as_str() != providers::STARS
            || payment.status != PaymentStatus::Pending
            || query.currency != "XTR"
            || payment.amount.minor != query.total_amount
            || payment
                .expires_at
                .is_some_and(|at| at < self.services.now())
        {
            return Err(ServiceError::rejected(
                "stars.invoice_mismatch",
                "счёт устарел, оформите заново",
            ));
        }
        let owner = conn
            .clients()
            .find_by_identity(
                sn_domain::client::IdentityKind::Telegram,
                &query.telegram_user_id.to_string(),
            )
            .await?;
        if owner.as_ref().map(|c| c.id) != Some(payment.client_id) {
            return Err(ServiceError::rejected(
                "stars.foreign_invoice",
                "счёт выставлен другому аккаунту",
            ));
        }
        if owner.is_some_and(|c| c.is_blocked()) {
            return Err(ServiceError::Forbidden);
        }
        Ok(())
    }

    pub async fn stars_payment(&self, payment: &StarsPayment) -> ServiceResult<AcceptResult> {
        let provider = self
            .services
            .payments()
            .get(providers::STARS)
            .ok_or(ServiceError::NotFound("провайдер stars"))?;
        let request = WebhookRequest {
            headers: Vec::new(),
            body: serde_json::to_vec(payment)
                .map_err(|e| ServiceError::Internal(e.to_string()))?
                .into(),
            remote_ip: None,
            route_suffix: None,
            query: None,
        };
        let outcome = provider
            .parse_webhook(&self.services.payments().config(providers::STARS), &request)
            .await?;
        self.apply_outcome(providers::STARS, &outcome).await
    }
}
