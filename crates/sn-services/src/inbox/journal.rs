use super::masking::{MaskedBody, body_event, mask_body};
use super::renewal::UNKNOWN_RENEWAL;
use super::{PaymentInbox, REJECTED_WEBHOOK_RETENTION_DAYS, WEBHOOK_RETENTION_DAYS, amount_text};
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::format;
use sn_domain::ids::{AdminId, ClientId, PaymentId};
use sn_domain::money::{Amount, Currency};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::{NoticeKind, Payment, PaymentStatus};
use sn_payments::{
    OutcomeSignal, PaymentError, PaymentRef, ProviderKind, ProviderStatus, WebhookOutcome,
    WebhookRequest,
};
use sn_store::{
    ActorKind, AuditEntry, ClientPerson, ClosedCredit, Db, InboxAttention, InboxOutcome,
    JournalEntry, JournalFilter, JournalRow, TrailKind, TrailStep,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const RETRY_STALE_MINUTES: i64 = 10;
pub const ATTENTION_HOURS: i64 = 24;
pub const RETRY_REVERSED: &str = "webhook.retry_reversed";
pub const RETRY_UNCONFIRMED: &str = "webhook.retry_unconfirmed";
pub const RETRY_NOTICE_CLOSED: &str = "webhook.retry_notice_closed";
const WEBHOOK_RETRIED_AUDIT: &str = "payment.webhook_retried";
const WEBHOOK_RETRY_STARTED_AUDIT: &str = "payment.webhook_retry_started";
const RETRY_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const SEARCH_LIMIT: usize = 200;

fn credits(outcome: &WebhookOutcome) -> bool {
    matches!(
        outcome.status,
        ProviderStatus::Paid
            | ProviderStatus::Overpaid { .. }
            | ProviderStatus::Underpaid { .. }
            | ProviderStatus::Held
    )
}

fn reversed(refund: bool) -> ServiceError {
    ServiceError::rejected(
        RETRY_REVERSED,
        if refund {
            "Повторить нельзя: платёжка сообщила о возврате по этому платежу. Сверьте платёж в её \
             кабинете. Если деньги всё же пришли, отметьте счёт оплаченным вручную."
        } else {
            "Повторить нельзя: после этого уведомления платёжка сообщила, что платёж отменён или не \
             прошёл. Сверьте платёж в её кабинете. Если деньги всё же пришли, отметьте счёт \
             оплаченным вручную."
        },
    )
}

fn notice_closed(closed: &ClosedCredit) -> ServiceError {
    let date = format::date_in(closed.resolved_at.date_naive(), Locale::Ru);
    let closed_text = match closed
        .resolution
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
    {
        Some(note) => format!("закрыто {date} с пометкой «{note}»"),
        None => format!("закрыто {date}"),
    };
    ServiceError::rejected(
        RETRY_NOTICE_CLOSED,
        format!(
            "Повторить нельзя: оповещение о ручном зачислении этого списания {closed_text}. \
             Если деньги уже зачислены, повтор зачислит их второй раз. Если не зачислены, \
             зачислите их вручную."
        ),
    )
}

fn unavailable(row: &JournalRow, now: DateTime<Utc>) -> ServiceError {
    let state = retry_state(row, now);
    let busy = row.outcome == InboxOutcome::Received;
    ServiceError::rejected(
        "webhook.retry_unavailable",
        state.note.unwrap_or_else(|| {
            if busy {
                "Уведомление сейчас обрабатывается, подождите несколько секунд.".to_string()
            } else {
                "Повторная обработка нужна только уведомлениям, которые упали с ошибкой. \
                 Это уведомление уже обработано."
                    .to_string()
            }
        }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroup {
    Credited,
    Repeat,
    Skipped,
    Rejected,
    Mismatch,
    Failed,
    Attention,
}

impl OutcomeGroup {
    pub fn outcomes(self) -> Vec<InboxOutcome> {
        match self {
            OutcomeGroup::Credited => vec![InboxOutcome::Accepted],
            OutcomeGroup::Repeat => vec![InboxOutcome::Duplicate],
            OutcomeGroup::Skipped => vec![InboxOutcome::Ignored, InboxOutcome::PaymentNotFound],
            OutcomeGroup::Rejected => vec![
                InboxOutcome::RejectedSignature,
                InboxOutcome::RejectedPayload,
                InboxOutcome::RejectedBusiness,
                InboxOutcome::NotAccepted,
            ],
            OutcomeGroup::Mismatch => vec![InboxOutcome::AmountMismatch],
            OutcomeGroup::Failed => vec![InboxOutcome::Error, InboxOutcome::Received],
            OutcomeGroup::Attention => vec![
                InboxOutcome::RejectedSignature,
                InboxOutcome::RejectedPayload,
                InboxOutcome::RejectedBusiness,
                InboxOutcome::NotAccepted,
                InboxOutcome::AmountMismatch,
                InboxOutcome::Error,
                InboxOutcome::Received,
            ],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct JournalQuery {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub outcome: Option<OutcomeGroup>,
    #[serde(default)]
    pub from: Option<DateTime<Utc>>,
    #[serde(default)]
    pub to: Option<DateTime<Utc>>,
    #[serde(default)]
    pub q: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Ok,
    Muted,
    Warn,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalPaymentView {
    pub id: PaymentId,
    pub public_id: Uuid,
    pub amount: Amount,
    pub amount_text: String,
    pub status: PaymentStatus,
    pub order_public_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetryState {
    pub available: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JournalItem {
    pub id: i64,
    pub provider: String,
    pub provider_title: String,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub event: Option<String>,
    pub outcome: InboxOutcome,
    pub tone: Tone,
    pub title: String,
    pub note: Option<String>,
    pub amount: Option<Amount>,
    pub amount_text: Option<String>,
    pub txid: Option<String>,
    pub payment: Option<JournalPaymentView>,
    pub client_id: Option<ClientId>,
    pub client: Option<ClientPerson>,
    pub deliveries: i32,
    pub retry: RetryState,
    pub legacy_path: bool,
    pub legacy_invoice: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderOption {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Retention {
    pub days: i64,
    pub rejected_days: i64,
}

impl Retention {
    pub const CURRENT: Retention = Retention {
        days: WEBHOOK_RETENTION_DAYS,
        rejected_days: REJECTED_WEBHOOK_RETENTION_DAYS,
    };
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JournalPage {
    pub items: Vec<JournalItem>,
    pub next: Option<i64>,
    pub providers: Vec<ProviderOption>,
    pub retention: Retention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fact {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepView {
    pub at: DateTime<Utc>,
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BodyView {
    Json { text: String },
    Form { fields: Vec<Fact> },
    Text { text: String },
    Binary { size: usize },
    Missing { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JournalCard {
    #[serde(flatten)]
    pub item: JournalItem,
    pub remote_ip: Option<String>,
    pub raw_event: Option<String>,
    pub advice: Option<String>,
    pub facts: Vec<Fact>,
    pub headers: Vec<Fact>,
    pub body: BodyView,
    pub masked: bool,
    pub steps: Vec<StepView>,
    pub retention: Retention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookAttention {
    pub rejected: i64,
    pub failed: i64,
    pub providers: Vec<String>,
    pub hours: i64,
}

pub fn exact(amount: Amount) -> String {
    match amount.as_money() {
        Some(money) if money.currency == Currency::Rub && money.minor % 100 != 0 => {
            let sign = if money.minor < 0 { "-" } else { "" };
            let abs = money.minor.unsigned_abs();
            format!(
                "{sign}{},{:02}{}₽",
                format::group_digits(abs / 100),
                abs % 100,
                format::NBSP
            )
        }
        _ => amount_text(amount),
    }
}

fn outcome_of(parsed: Option<&serde_json::Value>) -> Option<WebhookOutcome> {
    parsed.and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn status_word(code: &str) -> Option<&'static str> {
    Some(match code {
        "pending" => "ждёт оплаты",
        "paid" => "оплачен",
        "failed" => "не прошёл",
        "canceled" => "отменён",
        "refunded" => "возвращён",
        "partially_refunded" => "частично возвращён",
        _ => return None,
    })
}

fn readable(reason: &str) -> String {
    if let Some(rest) = reason.strip_prefix("error returned from database: ") {
        return format!("ошибка базы данных: {rest}");
    }
    if let Some(rest) = reason.strip_prefix("error communicating with database: ") {
        return format!("нет связи с базой данных: {rest}");
    }
    if reason.starts_with("pool timed out") {
        return "база данных не ответила вовремя".into();
    }
    if let Some(code) = reason.strip_prefix("платёж уже ")
        && let Some(word) = status_word(code.trim())
    {
        return format!("платёж уже {word}");
    }
    if let Some(code) = reason.strip_prefix("возврат по платежу в статусе ")
        && let Some(word) = status_word(code.trim())
    {
        return format!("возврат по платежу, который {word}");
    }
    reason.to_string()
}

const ROUTE_EVENTS: [(&str, &str); 6] = [
    ("check", "Проверка перед оплатой"),
    ("pay", "Оплата"),
    ("fail", "Оплата не прошла"),
    ("refund", "Возврат"),
    ("recurrent", "Автоплатёж"),
    ("cancel", "Отмена платежа"),
];

fn service_event(raw: Option<&str>) -> String {
    match raw {
        Some(raw) => ROUTE_EVENTS
            .iter()
            .find(|(code, _)| *code == raw)
            .map_or_else(
                || format!("Событие «{raw}»"),
                |(_, title)| (*title).to_string(),
            ),
        None => "Служебное событие".into(),
    }
}

fn event_text(outcome: Option<&WebhookOutcome>, raw: Option<&str>) -> Option<String> {
    match outcome {
        Some(o)
            if o.status == ProviderStatus::Pending
                && o.signal.is_none()
                && (o.payment.is_none()
                    || raw.is_some_and(|r| ROUTE_EVENTS.iter().any(|(code, _)| *code == r))) =>
        {
            Some(service_event(raw))
        }
        Some(o) => Some(event_label(o)),
        None => raw.map(|r| service_event(Some(r))),
    }
}

pub fn event_label(outcome: &WebhookOutcome) -> String {
    if let Some(OutcomeSignal::Notice { notice }) = &outcome.signal {
        return notice.kind.title_ru().to_string();
    }
    if matches!(outcome.signal, Some(OutcomeSignal::Precheck { .. })) {
        return "Проверка перед оплатой".into();
    }
    if outcome.method_revoked() {
        return "Отзыв разрешения на автоплатёж".into();
    }
    match outcome.status {
        ProviderStatus::Pending => "Платёж в процессе",
        ProviderStatus::Paid => "Оплата",
        ProviderStatus::Failed => "Оплата не прошла",
        ProviderStatus::Canceled => "Отмена платежа",
        ProviderStatus::Refunded => "Возврат",
        ProviderStatus::PartiallyRefunded { .. } => "Частичный возврат",
        ProviderStatus::RefundDeclined => "Отказ в возврате",
        ProviderStatus::Underpaid { .. } => "Оплата не полностью",
        ProviderStatus::Overpaid { .. } => "Оплата с излишком",
        ProviderStatus::Held => "Оплата задержана проверкой",
    }
    .into()
}

fn stale(row: &JournalRow, now: DateTime<Utc>) -> bool {
    row.outcome == InboxOutcome::Received
        && row.claimed_at.unwrap_or(row.received_at) < now - Duration::minutes(RETRY_STALE_MINUTES)
}

fn mismatch_note(row: &JournalRow, outcome: Option<&WebhookOutcome>) -> Option<String> {
    let invoiced = row.payment.as_ref().map(|p| p.amount);
    match (outcome.map(|o| o.amount), invoiced) {
        (Some(None), _) => Some("платёжка не подтвердила сумму".into()),
        (Some(Some(reported)), Some(invoiced)) => Some(format!(
            "в уведомлении {}, по счёту {}",
            exact(reported),
            exact(invoiced)
        )),
        _ => row.error.clone(),
    }
}

#[cfg(test)]
pub(super) fn verdict_note(row: &JournalRow) -> Option<String> {
    verdict(row, None, Utc::now()).2
}

fn verdict(
    row: &JournalRow,
    outcome: Option<&WebhookOutcome>,
    now: DateTime<Utc>,
) -> (Tone, String, Option<String>) {
    let note = row.error.as_deref().map(readable);
    match row.outcome {
        InboxOutcome::Accepted => (Tone::Ok, "Засчитано".into(), None),
        InboxOutcome::Duplicate => (Tone::Muted, "Повтор, уже было".into(), None),
        InboxOutcome::Ignored => (
            Tone::Muted,
            "Пропущено".into(),
            Some(note.unwrap_or_else(|| "событие не требует действий".into())),
        ),
        InboxOutcome::PaymentNotFound => (
            Tone::Muted,
            "Пропущено".into(),
            Some("событие без нашего платежа".into()),
        ),
        InboxOutcome::RejectedSignature => (
            Tone::Bad,
            "Отклонено".into(),
            Some("неверная подпись".into()),
        ),
        InboxOutcome::RejectedPayload => (
            Tone::Bad,
            "Отклонено".into(),
            Some(match note {
                Some(text) => format!("не удалось разобрать: {text}"),
                None => "не удалось разобрать".into(),
            }),
        ),
        InboxOutcome::NotAccepted => (
            Tone::Bad,
            "Отклонено".into(),
            Some(note.unwrap_or_else(|| "платёжка не принимает уведомления".into())),
        ),
        InboxOutcome::RejectedBusiness => (Tone::Bad, "Отклонено".into(), note),
        InboxOutcome::AmountMismatch => (
            Tone::Bad,
            "Сумма не сошлась".into(),
            mismatch_note(row, outcome),
        ),
        InboxOutcome::Error => (Tone::Bad, "Ошибка обработки".into(), note),
        InboxOutcome::Received if stale(row, now) => (
            Tone::Bad,
            "Ошибка обработки".into(),
            Some("обработка не завершилась".into()),
        ),
        InboxOutcome::Received => (Tone::Warn, "Обрабатывается".into(), None),
    }
}

fn waits_for_contract(row: &JournalRow) -> bool {
    row.outcome == InboxOutcome::Ignored && row.error.as_deref() == Some(UNKNOWN_RENEWAL)
}

fn retry_state(row: &JournalRow, now: DateTime<Utc>) -> RetryState {
    let failed = row.outcome == InboxOutcome::Error || stale(row, now) || waits_for_contract(row);
    if !failed {
        return RetryState {
            available: false,
            note: None,
        };
    }
    if row.parsed.is_none() {
        return RetryState {
            available: false,
            note: Some(
                "Повторить отсюда нельзя: уведомление не прошло проверку, поэтому не сохранялось. \
                 Мы ответили платёжке ошибкой, и она пришлёт его ещё раз сама."
                    .into(),
            ),
        };
    }
    RetryState {
        available: true,
        note: None,
    }
}

fn reported_amount(outcome: Option<&WebhookOutcome>, row: &JournalRow) -> Option<Amount> {
    let from_outcome = outcome.and_then(|o| match o.status {
        ProviderStatus::Underpaid { received } | ProviderStatus::Overpaid { received } => {
            Some(received)
        }
        ProviderStatus::PartiallyRefunded { refunded } => Some(refunded),
        _ => o.amount,
    });
    from_outcome.or_else(|| row.payment.as_ref().map(|p| p.amount))
}

impl PaymentInbox<'_> {
    fn provider_title(&self, id: &str) -> String {
        self.services
            .payments()
            .get(id)
            .map_or_else(|| id.to_string(), |p| p.title().to_string())
    }

    fn provider_options(&self) -> Vec<ProviderOption> {
        self.services
            .payments()
            .providers()
            .iter()
            .filter(|p| p.kind() == ProviderKind::External && p.accepts_http_webhooks())
            .map(|p| ProviderOption {
                id: p.id().to_string(),
                title: p.title().to_string(),
            })
            .collect()
    }

    fn journal_item(
        &self,
        row: JournalRow,
        people: &HashMap<ClientId, ClientPerson>,
        now: DateTime<Utc>,
    ) -> JournalItem {
        let outcome = outcome_of(row.parsed.as_ref());
        let (tone, title, note) = verdict(&row, outcome.as_ref(), now);
        let amount = reported_amount(outcome.as_ref(), &row);
        let retry = retry_state(&row, now);
        let payment = match (row.payment_id, row.payment.as_ref()) {
            (Some(id), Some(p)) => Some(JournalPaymentView {
                id,
                public_id: p.public_id,
                amount: p.amount,
                amount_text: exact(p.amount),
                status: p.status,
                order_public_id: p.order_public_id,
            }),
            _ => None,
        };
        JournalItem {
            id: row.id,
            provider_title: self.provider_title(&row.source),
            provider: row.source,
            received_at: row.received_at,
            processed_at: row.processed_at,
            event: event_text(outcome.as_ref(), row.event.as_deref()),
            outcome: row.outcome,
            tone,
            title,
            note,
            amount_text: amount.map(exact),
            amount,
            txid: outcome.as_ref().and_then(|o| o.provider_txid.clone()),
            client: row.client_id.and_then(|id| people.get(&id).cloned()),
            client_id: row.client_id,
            payment,
            deliveries: row.deliveries,
            retry,
            legacy_path: row.legacy_path,
            legacy_invoice: row.legacy_invoice,
        }
    }

    pub async fn journal(
        &self,
        query: &JournalQuery,
        page: PageRequest,
    ) -> ServiceResult<JournalPage> {
        let filter = JournalFilter {
            source: query
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string),
            outcomes: query
                .outcome
                .map(OutcomeGroup::outcomes)
                .unwrap_or_default(),
            since: query.from,
            until: query.to,
            search: query
                .q
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(|q| q.chars().take(SEARCH_LIMIT).collect()),
        };
        let mut conn = self.services.store().conn().await?;
        let rows = conn
            .webhooks()
            .journal(&filter, page.after, page.fetch_limit())
            .await?;
        let Page { items: rows, next } = Page::from_overfetch(rows, page, |r| r.id);
        let ids: Vec<ClientId> = rows
            .iter()
            .filter_map(|r| r.client_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let people = conn.clients().people(&ids).await?;
        drop(conn);
        let now = self.services.now();
        Ok(JournalPage {
            items: rows
                .into_iter()
                .map(|row| self.journal_item(row, &people, now))
                .collect(),
            next,
            providers: self.provider_options(),
            retention: Retention::CURRENT,
        })
    }

    pub async fn journal_card(&self, id: i64) -> ServiceResult<JournalCard> {
        let mut conn = self.services.store().conn().await?;
        let entry = conn
            .webhooks()
            .journal_entry(id)
            .await?
            .ok_or(ServiceError::NotFound("уведомление"))?;
        let ids: Vec<ClientId> = entry.row.client_id.into_iter().collect();
        let people = conn.clients().people(&ids).await?;
        let admins: HashMap<AdminId, String> = if entry.trail.iter().any(|s| s.admin_id.is_some()) {
            conn.admins()
                .list()
                .await?
                .into_iter()
                .map(|a| (a.id, a.email))
                .collect()
        } else {
            HashMap::new()
        };
        drop(conn);
        Ok(self.card(entry, &people, &admins))
    }

    fn card(
        &self,
        entry: JournalEntry,
        people: &HashMap<ClientId, ClientPerson>,
        admins: &HashMap<AdminId, String>,
    ) -> JournalCard {
        let now = self.services.now();
        let JournalEntry {
            row,
            headers,
            body,
            trail,
        } = entry;
        let outcome = outcome_of(row.parsed.as_ref());
        let advice = advice(&row, now);
        let facts = facts(outcome.as_ref(), &row);
        let steps = steps(&row, &trail, admins, now);
        let (body, masked, body_event) = body_view(&row, body.as_deref());
        let raw_event = row.event.clone().or(body_event);
        let remote_ip = row.remote_ip.clone();
        JournalCard {
            item: self.journal_item(row, people, now),
            remote_ip,
            raw_event,
            advice,
            facts,
            headers: header_facts(&headers),
            body,
            masked,
            steps,
            retention: Retention::CURRENT,
        }
    }

    async fn retry_entry(&self, id: i64) -> ServiceResult<JournalEntry> {
        let mut conn = self.services.store().conn().await?;
        conn.webhooks()
            .journal_entry(id)
            .await?
            .ok_or(ServiceError::NotFound("уведомление"))
    }

    async fn retry_guard(&self, row: &JournalRow, outcome: &WebhookOutcome) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        if let Some(closed) = conn
            .payment_notices()
            .closed_credit(&row.source, &outcome.event_key)
            .await?
        {
            return Err(notice_closed(&closed));
        }
        let payment = match &outcome.payment {
            Some(reference) => super::lookup(&mut conn, &row.source, reference).await?,
            None => None,
        };
        if let Some(payment) = &payment {
            let txid = outcome
                .provider_txid
                .as_deref()
                .or(payment.provider_txid.as_deref());
            if let Some(kind) = conn
                .webhooks()
                .reversal(row.id, &row.source, payment.id, txid)
                .await?
            {
                return Err(reversed(matches!(
                    kind.as_str(),
                    "refunded" | "partially_refunded"
                )));
            }
            if conn
                .payment_notices()
                .for_payment(payment.id)
                .await?
                .iter()
                .any(|notice| {
                    matches!(
                        notice.kind,
                        NoticeKind::RefundReported | NoticeKind::Chargeback
                    )
                })
            {
                return Err(reversed(true));
            }
        }
        drop(conn);
        self.recheck(&row.source, outcome, payment.as_ref()).await
    }

    async fn recheck(
        &self,
        source: &str,
        outcome: &WebhookOutcome,
        payment: Option<&Payment>,
    ) -> ServiceResult<()> {
        let Some(provider) = self
            .services
            .payments()
            .get(source)
            .filter(|p| p.kind() == ProviderKind::External)
        else {
            return Ok(());
        };
        if let Some(PaymentRef::Renewal(renewal)) = &outcome.payment {
            let config = self.services.payments().config(provider.id());
            let confirmed = tokio::time::timeout(
                RETRY_CHECK_TIMEOUT,
                provider.confirm_renewal(&config, &renewal.series, &renewal.txid),
            )
            .await;
            return match confirmed {
                Ok(Ok(Some(fresh)))
                    if matches!(
                        fresh.status,
                        ProviderStatus::Paid | ProviderStatus::Overpaid { .. }
                    ) =>
                {
                    Ok(())
                }
                Ok(Ok(_)) => Err(ServiceError::rejected(
                    RETRY_UNCONFIRMED,
                    format!(
                        "Повторить нельзя: {} не подтверждает это списание как оплаченное \
                         продление. Сверьте платёж в её кабинете.",
                        provider.title()
                    ),
                )),
                Ok(Err(
                    PaymentError::NotImplemented(_)
                    | PaymentError::Unsupported(_)
                    | PaymentError::NotConfigured(_),
                )) => Ok(()),
                Ok(Err(err)) => Err(ServiceError::rejected(
                    RETRY_UNCONFIRMED,
                    format!(
                        "Не удалось спросить у {} статус списания: {err}. Попробуйте позже.",
                        provider.title()
                    ),
                )),
                Err(_) => Err(ServiceError::rejected(
                    RETRY_UNCONFIRMED,
                    format!(
                        "{} не ответила на запрос статуса списания. Попробуйте позже.",
                        provider.title()
                    ),
                )),
            };
        }
        let external = payment
            .filter(|_| provider.status_by_order())
            .map(|p| p.public_id.to_string())
            .or_else(|| outcome.provider_txid.clone())
            .or_else(|| payment.and_then(|p| p.provider_txid.clone()))
            .filter(|txid| !txid.trim().is_empty())
            .or_else(|| {
                payment
                    .filter(|_| provider.polls_by_payment_id())
                    .map(|p| p.public_id.to_string())
            });
        let Some(external) = external else {
            return Ok(());
        };
        let config = self.services.payments().config(provider.id());
        let fresh = match tokio::time::timeout(
            RETRY_CHECK_TIMEOUT,
            provider.fetch_status(&config, &external),
        )
        .await
        {
            Ok(Ok(Some(fresh))) => fresh,
            Ok(Ok(None))
            | Ok(Err(PaymentError::NotImplemented(_)))
            | Ok(Err(PaymentError::Unsupported(_)))
            | Ok(Err(PaymentError::NotConfigured(_))) => return Ok(()),
            Ok(Err(err)) => {
                return Err(ServiceError::rejected(
                    RETRY_UNCONFIRMED,
                    format!(
                        "Не удалось спросить у {} статус платежа: {err}. Попробуйте позже.",
                        provider.title()
                    ),
                ));
            }
            Err(_) => {
                return Err(ServiceError::rejected(
                    RETRY_UNCONFIRMED,
                    format!(
                        "{} не ответила на запрос статуса платежа. Попробуйте позже.",
                        provider.title()
                    ),
                ));
            }
        };
        match fresh.status {
            ProviderStatus::Paid
            | ProviderStatus::Overpaid { .. }
            | ProviderStatus::Held
            | ProviderStatus::RefundDeclined => Ok(()),
            ProviderStatus::Underpaid { .. }
                if matches!(outcome.status, ProviderStatus::Underpaid { .. }) =>
            {
                Ok(())
            }
            ProviderStatus::Refunded | ProviderStatus::PartiallyRefunded { .. } => {
                Err(reversed(true))
            }
            ProviderStatus::Canceled | ProviderStatus::Failed => Err(reversed(false)),
            ProviderStatus::Pending | ProviderStatus::Underpaid { .. } => {
                Err(ServiceError::rejected(
                    RETRY_UNCONFIRMED,
                    format!(
                        "Повторить пока нельзя: {} сейчас не подтверждает эту оплату. \
                         Сверьте платёж в её кабинете.",
                        provider.title()
                    ),
                ))
            }
        }
    }

    pub async fn retry_webhook(&self, id: i64, admin: AdminId) -> ServiceResult<JournalCard> {
        let now = self.services.now();
        let entry = self.retry_entry(id).await?;
        if !retry_state(&entry.row, now).available {
            return Err(unavailable(&entry.row, now));
        }
        if let Some(outcome) = entry
            .row
            .parsed
            .clone()
            .and_then(|parsed| serde_json::from_value::<WebhookOutcome>(parsed).ok())
            .filter(credits)
        {
            self.retry_guard(&entry.row, &outcome).await?;
        }
        let claim = {
            let mut tx = self.services.store().begin().await?;
            let claim = tx
                .webhooks()
                .claim_retry(
                    id,
                    admin,
                    now - Duration::minutes(RETRY_STALE_MINUTES),
                    UNKNOWN_RENEWAL,
                )
                .await?;
            if let Some(claim) = &claim {
                tx.audit()
                    .record(&AuditEntry {
                        actor_kind: ActorKind::Admin,
                        actor_id: Some(admin.0),
                        action: WEBHOOK_RETRY_STARTED_AUDIT.into(),
                        entity_kind: Some("webhook".into()),
                        entity_id: Some(id),
                        details: serde_json::json!({
                            "provider": claim.source,
                            "payment_id": entry.row.payment_id,
                        }),
                        ip: None,
                    })
                    .await?;
            }
            tx.commit().await?;
            claim
        };
        let Some(claim) = claim else {
            let entry = self.retry_entry(id).await?;
            return Err(unavailable(&entry.row, now));
        };
        let mut legacy_invoice = None;
        let settled = match serde_json::from_value::<WebhookOutcome>(claim.parsed) {
            Ok(mut outcome) => {
                if outcome
                    .saved_method
                    .as_ref()
                    .is_some_and(|m| m.token.is_empty())
                {
                    outcome.saved_method = None;
                }
                let result = self.apply_outcome(&claim.source, &outcome).await;
                let mut settled = super::Settled::from(result);
                let request = WebhookRequest {
                    headers: Vec::new(),
                    body: claim.body.unwrap_or_default().into(),
                    remote_ip: None,
                    route_suffix: None,
                    query: None,
                };
                legacy_invoice = self
                    .settle_legacy(&claim.source, &outcome, &request, &mut settled, Some(admin))
                    .await;
                if settled.payment_id.is_none() {
                    settled.payment_id = self.linked_payment(&claim.source, &outcome).await;
                }
                settled
            }
            Err(err) => super::Settled {
                outcome: InboxOutcome::Error,
                payment_id: None,
                error: Some(format!("сохранённое уведомление не читается: {err}")),
                failed: true,
            },
        };
        {
            let mut conn = self.services.store().conn().await?;
            conn.webhooks()
                .finish(
                    id,
                    settled.outcome,
                    settled.payment_id,
                    settled.error.as_deref(),
                    Some(admin),
                )
                .await?;
            conn.audit()
                .record(&AuditEntry {
                    actor_kind: ActorKind::Admin,
                    actor_id: Some(admin.0),
                    action: WEBHOOK_RETRIED_AUDIT.into(),
                    entity_kind: Some("webhook".into()),
                    entity_id: Some(id),
                    details: serde_json::json!({
                        "provider": claim.source,
                        "outcome": settled.outcome.as_str(),
                        "payment_id": settled.payment_id,
                        "error": settled.error,
                        "legacy_invoice": legacy_invoice,
                    }),
                    ip: None,
                })
                .await?;
        }
        self.mark_legacy(&claim.source, &claim.dedupe_key, false, legacy_invoice)
            .await;
        tracing::info!(webhook = id, admin = %admin, outcome = settled.outcome.as_str(), "уведомление платёжки обработано повторно");
        self.journal_card(id).await
    }

    pub async fn webhook_attention(&self) -> ServiceResult<WebhookAttention> {
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let InboxAttention {
            rejected,
            failed,
            sources,
        } = conn
            .webhooks()
            .attention(
                now - Duration::hours(ATTENTION_HOURS),
                now - Duration::minutes(RETRY_STALE_MINUTES),
            )
            .await?;
        let mut providers: Vec<String> = sources.iter().map(|s| self.provider_title(s)).collect();
        providers.sort();
        Ok(WebhookAttention {
            rejected,
            failed,
            providers,
            hours: ATTENTION_HOURS,
        })
    }
}

fn advice(row: &JournalRow, now: DateTime<Utc>) -> Option<String> {
    let text = match row.outcome {
        InboxOutcome::Error | InboxOutcome::Received if retry_state(row, now).available => {
            "Обработка упала из-за временной ошибки. Платёжка обычно присылает такое уведомление \
             ещё раз сама. Если повтора нет, нажмите «Обработать ещё раз»: деньги дважды не зачислятся."
        }
        InboxOutcome::Error => {
            "Уведомление не удалось проверить, поэтому оно не сохранялось и повторить его отсюда \
             нельзя. Мы ответили платёжке ошибкой, и она пришлёт его ещё раз сама. Если повторов \
             нет, проверьте платёж в кабинете платёжки."
        }
        InboxOutcome::Received => return None,
        InboxOutcome::RejectedSignature => {
            "Подпись не совпала с ключом из настроек платёжки. Если это настоящая платёжка, \
             проверьте секретный ключ в «Настройки → Оплата». Тело таких уведомлений не \
             сохраняется."
        }
        InboxOutcome::NotAccepted => {
            "Платёжка не настроена или не принимает уведомления по HTTP, поэтому уведомление \
             не принято. Выключенная, но настроенная платёжка уведомления принимает. Тело не \
             сохраняется."
        }
        InboxOutcome::RejectedPayload => {
            "Уведомление не похоже на то, что присылает платёжка, поэтому не принято. Тело не \
             сохраняется."
        }
        InboxOutcome::RejectedBusiness => {
            "Уведомление проверено, но противоречит данным панели, поэтому ничего не изменилось."
        }
        InboxOutcome::AmountMismatch => {
            "Сумма в уведомлении не совпала со счётом, поэтому оплата не засчитана. Проверьте \
             платёж в кабинете платёжки; если деньги пришли полностью, отметьте платёж \
             оплаченным в «Деньгах»."
        }
        InboxOutcome::PaymentNotFound => {
            "В панели нет платежа из этого уведомления. Так бывает, если на одном аккаунте \
             платёжки работают несколько магазинов."
        }
        InboxOutcome::Duplicate => {
            "Это событие уже было учтено раньше, повторно ничего не зачислено."
        }
        InboxOutcome::Ignored if waits_for_contract(row) => {
            "Платёжка списала деньги за продление подписки, которой нет в панели, поэтому ничего \
             не зачислено. Если договор этой подписки добавили позже, например повторным переносом \
             из старой панели, нажмите «Обработать ещё раз»: списание проведётся по нему. Иначе \
             зачислите деньги вручную."
        }
        InboxOutcome::Accepted | InboxOutcome::Ignored => return None,
    };
    Some(text.to_string())
}

fn reference_text(reference: &PaymentRef) -> String {
    match reference {
        PaymentRef::Payment(id) => format!("№{id}"),
        PaymentRef::PublicId(id) => id.to_string(),
        PaymentRef::ProviderTxid(txid) => format!("транзакция {txid}"),
        PaymentRef::Buyer(buyer) => format!(
            "покупатель с Telegram ID {}, {}",
            buyer.telegram_id, buyer.txid
        ),
        PaymentRef::Renewal(renewal) => {
            format!("продление {} по подписке {}", renewal.txid, renewal.series)
        }
    }
}

fn legacy_facts(row: &JournalRow) -> Vec<Fact> {
    let mut facts = Vec::new();
    if row.legacy_path {
        facts.push(Fact {
            label: "Адрес".into(),
            value: "старый адрес старой панели — платёжка ещё шлёт уведомления туда".into(),
        });
    }
    if row.legacy_invoice {
        facts.push(Fact {
            label: "Счёт".into(),
            value: "счёт старой панели, оплачен после переезда".into(),
        });
    }
    facts
}

fn facts(outcome: Option<&WebhookOutcome>, row: &JournalRow) -> Vec<Fact> {
    let Some(outcome) = outcome else {
        return legacy_facts(row);
    };
    let mut facts = vec![Fact {
        label: "Событие".into(),
        value: event_text(Some(outcome), row.event.as_deref())
            .unwrap_or_else(|| event_label(outcome)),
    }];
    facts.extend(legacy_facts(row));
    let mut push = |label: &str, value: String| {
        facts.push(Fact {
            label: label.into(),
            value,
        });
    };
    match outcome.status {
        ProviderStatus::Underpaid { received } => {
            push("Получено", exact(received));
        }
        ProviderStatus::Overpaid { received } => {
            push("Получено", exact(received));
        }
        ProviderStatus::PartiallyRefunded { refunded } => {
            push("Возвращено", exact(refunded));
        }
        _ => {}
    }
    if let Some(amount) = outcome.amount {
        push("Сумма в уведомлении", exact(amount));
    }
    if let Some(payment) = &row.payment {
        push("Сумма счёта", exact(payment.amount));
    }
    if let Some(reference) = &outcome.payment {
        push("Платёж в уведомлении", reference_text(reference));
    }
    if let Some(txid) = &outcome.provider_txid {
        push("Транзакция платёжки", txid.clone());
    }
    if !outcome.event_key.is_empty() {
        push("Ключ события", outcome.event_key.clone());
    }
    if let Some(OutcomeSignal::Notice { notice }) = &outcome.signal {
        push("Сообщение платёжки", notice.text.clone());
    }
    if let Some(method) = &outcome.saved_method {
        push(
            "Карта для автоплатежей",
            method.title.clone().unwrap_or_else(|| "сохраняется".into()),
        );
    }
    facts
}

fn header_facts(headers: &serde_json::Value) -> Vec<Fact> {
    headers
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| Fact {
                    label: key.clone(),
                    value: value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn body_view(row: &JournalRow, body: Option<&[u8]>) -> (BodyView, bool, Option<String>) {
    let Some(body) = body else {
        let reason = if row.outcome.is_unverified() {
            "Тело не сохраняется: уведомление не прошло проверку".to_string()
        } else {
            "Тело не сохранено".to_string()
        };
        return (BodyView::Missing { reason }, false, None);
    };
    let (masked, changed) = mask_body(body);
    let event = body_event(&masked);
    let view = match masked {
        MaskedBody::Json(value) => BodyView::Json {
            text: serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
        },
        MaskedBody::Form(pairs) => BodyView::Form {
            fields: pairs
                .into_iter()
                .map(|(label, value)| Fact { label, value })
                .collect(),
        },
        MaskedBody::Text(text) => BodyView::Text { text },
        MaskedBody::Binary(size) => BodyView::Binary { size },
    };
    let stored_masked = changed || body_has_mask(body);
    (view, stored_masked, event)
}

fn body_has_mask(body: &[u8]) -> bool {
    std::str::from_utf8(body).is_ok_and(|text| text.contains('•') || text.contains("%E2%80%A2"))
}

fn outcome_step(outcome: InboxOutcome, note: Option<&str>) -> (String, Tone) {
    let note = note.map(readable);
    let (tone, text) = match outcome {
        InboxOutcome::Accepted => (Tone::Ok, "Засчитано".to_string()),
        InboxOutcome::Duplicate => (Tone::Muted, "Повтор: событие уже было учтено".to_string()),
        InboxOutcome::Ignored => (Tone::Muted, "Пропущено".to_string()),
        InboxOutcome::PaymentNotFound => (
            Tone::Muted,
            "Пропущено: событие без нашего платежа".to_string(),
        ),
        InboxOutcome::RejectedSignature => (Tone::Bad, "Отклонено: неверная подпись".to_string()),
        InboxOutcome::RejectedPayload => (Tone::Bad, "Отклонено: не удалось разобрать".to_string()),
        InboxOutcome::NotAccepted | InboxOutcome::RejectedBusiness => {
            (Tone::Bad, "Отклонено".to_string())
        }
        InboxOutcome::AmountMismatch => return ("Сумма не сошлась".to_string(), Tone::Bad),
        InboxOutcome::Error => (Tone::Bad, "Ошибка обработки".to_string()),
        InboxOutcome::Received => (Tone::Warn, "Обрабатывается".to_string()),
    };
    match note {
        Some(note) if !note.is_empty() && !text.contains(&note) => {
            (format!("{text}: {note}"), tone)
        }
        _ => (text, tone),
    }
}

fn steps(
    row: &JournalRow,
    trail: &[TrailStep],
    admins: &HashMap<AdminId, String>,
    now: DateTime<Utc>,
) -> Vec<StepView> {
    let mut steps = vec![StepView {
        at: row.received_at,
        text: match &row.remote_ip {
            Some(ip) => format!("Получено с адреса {ip}"),
            None => "Получено".into(),
        },
        tone: Tone::Muted,
    }];
    let who = |admin: Option<AdminId>| {
        admin.map(|id| {
            admins
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("администратор {id}"))
        })
    };
    for step in trail {
        let view = match step.step {
            TrailKind::Delivery => StepView {
                at: step.at,
                text: "Платёжка прислала это уведомление ещё раз".into(),
                tone: Tone::Muted,
            },
            TrailKind::Retry => StepView {
                at: step.at,
                text: match who(step.admin_id) {
                    Some(name) => format!("{name} запускает повторную обработку"),
                    None => "Повторная обработка".into(),
                },
                tone: Tone::Muted,
            },
            TrailKind::Processed => {
                let (text, tone) = outcome_step(
                    step.outcome.unwrap_or(InboxOutcome::Error),
                    step.note.as_deref(),
                );
                StepView {
                    at: step.at,
                    text: match who(step.admin_id) {
                        Some(name) => format!("{text} (повторно, {name})"),
                        None => text,
                    },
                    tone,
                }
            }
        };
        steps.push(view);
    }
    if trail.is_empty()
        && let Some(at) = row.processed_at
    {
        let (text, tone) = outcome_step(row.outcome, row.error.as_deref());
        steps.push(StepView { at, text, tone });
    }
    if stale(row, now) {
        steps.push(StepView {
            at: row.claimed_at.unwrap_or(row.received_at),
            text: "Обработка не завершилась".into(),
            tone: Tone::Bad,
        });
    }
    steps
}
