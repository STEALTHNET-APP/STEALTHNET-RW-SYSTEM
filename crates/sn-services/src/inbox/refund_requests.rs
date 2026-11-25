use super::{AcceptResult, PaymentInbox, STORED_TEXT_LIMIT, clip, refunded_minor};
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sn_domain::ids::{AdminId, PaymentId};
use sn_domain::money::Amount;
use sn_domain::payment::{Payment, PaymentStatus, RefundSubscriptionAction};
use sn_store::{ActorKind, AuditEntry, Db, ProviderRefund, ProviderRefundStatus, Tx, audit_entity};

pub const PENDING_REFUNDS_MAX: i64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingRefundRequest {
    pub request_id: i64,
    pub payment_id: PaymentId,
    pub amount: Amount,
    pub subscription_action: RefundSubscriptionAction,
    pub reason: Option<String>,
    pub requested_by: Option<AdminId>,
    pub provider_refund_id: Option<String>,
    pub held: bool,
    pub requested_at: DateTime<Utc>,
    pub polled_at: Option<DateTime<Utc>>,
}

impl From<ProviderRefund> for PendingRefundRequest {
    fn from(request: ProviderRefund) -> Self {
        PendingRefundRequest {
            request_id: request.id,
            payment_id: request.payment_id,
            amount: request.amount,
            subscription_action: request.subscription_action,
            reason: request.reason,
            requested_by: request.admin_id,
            provider_refund_id: request.provider_refund_id,
            held: request.hold_entry_id.is_some(),
            requested_at: request.created_at,
            polled_at: request.polled_at,
        }
    }
}

fn explained(text: &str) -> ServiceResult<String> {
    let text = clip(text.trim(), STORED_TEXT_LIMIT);
    if text.is_empty() {
        return Err(ServiceError::rejected(
            "refund.request_note",
            "Напишите, почему заявка закрывается вручную",
        ));
    }
    Ok(text)
}

impl PaymentInbox<'_> {
    async fn pending_request(
        &self,
        tx: &mut Tx,
        request_id: i64,
    ) -> ServiceResult<(Payment, ProviderRefund)> {
        let found = tx.payments().provider_refund(request_id).await?;
        let payment = tx.payments().lock(found.payment_id).await?;
        let request = tx.payments().provider_refund(request_id).await?;
        if request.status != ProviderRefundStatus::Pending {
            return Err(ServiceError::rejected(
                "refund.request_resolved",
                "Заявка на возврат уже закрыта",
            ));
        }
        Ok((payment, request))
    }

    pub async fn pending_refund_requests(
        &self,
        limit: i64,
    ) -> ServiceResult<Vec<PendingRefundRequest>> {
        let mut conn = self.services.store().conn().await?;
        let mut requests: Vec<PendingRefundRequest> = conn
            .payments()
            .pending_provider_refunds(self.services.now(), limit.clamp(1, PENDING_REFUNDS_MAX))
            .await?
            .into_iter()
            .map(PendingRefundRequest::from)
            .collect();
        requests.sort_by_key(|request| (request.requested_at, request.request_id));
        Ok(requests)
    }

    pub async fn cancel_refund_request(
        &self,
        request_id: i64,
        admin: AdminId,
        reason: &str,
    ) -> ServiceResult<()> {
        let reason = explained(reason)?;
        let mut tx = self.services.store().begin().await?;
        let (payment, request) = self.pending_request(&mut tx, request_id).await?;
        tx.payments()
            .resolve_provider_refund(
                request.id,
                ProviderRefundStatus::Declined,
                None,
                Some(&format!("отменена администратором: {reason}")),
                self.services.now(),
            )
            .await?;
        self.release_hold(
            &mut tx,
            &payment,
            &request,
            request.amount.minor,
            request.bonus_minor,
        )
        .await?;
        self.resume_delivery(&mut tx, payment.order_id).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "payment.refund_canceled".into(),
                entity_kind: Some(audit_entity::PAYMENT.into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "request_id": request.id,
                    "amount": request.amount,
                    "provider_refund_id": request.provider_refund_id,
                    "released_hold": request.hold_entry_id.is_some(),
                    "reason": reason,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn confirm_refund_request(
        &self,
        request_id: i64,
        admin: AdminId,
        note: &str,
    ) -> ServiceResult<AcceptResult> {
        let note = explained(note)?;
        let mut tx = self.services.store().begin().await?;
        let (payment, request) = self.pending_request(&mut tx, request_id).await?;
        if !matches!(
            payment.status,
            PaymentStatus::Paid | PaymentStatus::PartiallyRefunded
        ) {
            return Err(ServiceError::rejected(
                "refund.not_paid",
                "Вернуть можно только оплаченный платёж",
            ));
        }
        let remaining = payment.amount.minor - refunded_minor(&mut tx, payment.id).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::Admin,
                actor_id: Some(admin.0),
                action: "payment.refund_confirmed_manually".into(),
                entity_kind: Some(audit_entity::PAYMENT.into()),
                entity_id: Some(payment.id.0),
                details: serde_json::json!({
                    "request_id": request.id,
                    "amount": request.amount,
                    "provider_refund_id": request.provider_refund_id,
                    "note": note,
                }),
                ip: None,
            })
            .await?;
        let event_key = request
            .provider_refund_id
            .clone()
            .unwrap_or_else(|| format!("manual:{}", request.id));
        self.confirm_request(tx, &payment, &request, &event_key, remaining)
            .await
    }
}
