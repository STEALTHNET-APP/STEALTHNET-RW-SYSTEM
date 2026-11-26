use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{ClientId, PaymentId};
use sn_domain::paging::{Page, PageRequest};
use sn_domain::payment::PaymentStatus;
use sn_services::admin::{
    CancelPaymentRequest, CancelPaymentResult, MarkPaidRequest, MarkPaidResult, PaymentFilter,
    RefundRequest,
};
use sn_services::decisions::{
    CHARGES_LIMIT, CREDITS_LIMIT, ChargeReviewItem, CreditView, RefundActionView, RefundRequestView,
};
use sn_services::inbox::{JournalCard, JournalPage, JournalQuery, OutcomeGroup, RefundOptions};

const REFUND_LIST_LIMIT: i64 = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/payments", get(list))
        .route("/payments/{id}/mark-paid", post(mark_paid))
        .route("/payments/{id}/cancel", post(cancel_payment))
        .route("/payments/{id}/refund", get(refund_options).post(refund))
        .route("/charges/review", get(charges_review))
        .route("/credits", get(credits))
        .route("/credits/{id}/close", post(close_credit))
        .route("/refunds/pending", get(pending_refunds))
        .route("/refunds/actions", get(refund_actions))
        .route("/refunds/{id}/cancel", post(cancel_refund_request))
        .route("/refunds/{id}/confirm", post(confirm_refund_request))
        .route("/refunds/{id}/action/retry", post(retry_refund_action))
        .route("/refunds/{id}/action/close", post(close_refund_action))
        .route("/webhooks", get(webhooks))
        .route("/webhooks/{id}", get(webhook))
        .route("/webhooks/{id}/retry", post(retry_webhook))
}

#[derive(Debug, Default, Deserialize)]
struct LimitQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct PendingQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    stuck: bool,
}

#[derive(Debug, Default, Deserialize)]
struct NoteRequest {
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ReasonRequest {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    client_id: Option<i64>,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

#[derive(Deserialize)]
struct WebhookListQuery {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    outcome: Option<OutcomeGroup>,
    #[serde(default)]
    from: Option<DateTime<Utc>>,
    #[serde(default)]
    to: Option<DateTime<Utc>>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<Page<sn_services::admin::PaymentListItem>>> {
    admin.require(Permission::Money)?;
    let status = match query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(value) => Some(PaymentStatus::parse(value).map_err(|_| {
            ApiError::bad_request("validation.status", format!("неизвестный статус {value}"))
        })?),
        None => None,
    };
    let filter = PaymentFilter {
        status,
        provider: query.provider,
        client_id: query.client_id.map(ClientId),
        id: query.id.map(PaymentId),
    };
    Ok(Json(
        state
            .services
            .admin()
            .payments_view(&filter, PageRequest::new(query.limit, query.after))
            .await?,
    ))
}

async fn mark_paid(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<MarkPaidRequest>>,
) -> ApiResult<Json<MarkPaidResult>> {
    admin.require(Permission::Money)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .admin()
            .mark_paid(PaymentId(id), &body, admin.0.admin_id)
            .await?,
    ))
}

async fn cancel_payment(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<CancelPaymentRequest>>,
) -> ApiResult<Json<CancelPaymentResult>> {
    admin.require(Permission::Money)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    Ok(Json(
        state
            .services
            .admin()
            .cancel_payment(PaymentId(id), &body, admin.0.admin_id)
            .await?,
    ))
}

async fn refund_options(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<RefundOptions>> {
    admin.require(Permission::Refunds)?;
    Ok(Json(
        state.services.inbox().refund_options(PaymentId(id)).await?,
    ))
}

async fn refund(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RefundRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let refund = state
        .services
        .admin()
        .refund(PaymentId(id), &body, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "refund": refund })))
}

async fn charges_review(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<ChargeReviewItem>>> {
    admin.require(Permission::Money)?;
    Ok(Json(
        state
            .services
            .decisions()
            .charges(query.limit.unwrap_or(CHARGES_LIMIT))
            .await?,
    ))
}

async fn credits(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<CreditView>>> {
    admin.require(Permission::Money)?;
    Ok(Json(
        state
            .services
            .decisions()
            .credits(query.limit.unwrap_or(CREDITS_LIMIT))
            .await?,
    ))
}

async fn close_credit(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<NoteRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Money)?;
    let note = body.and_then(|b| b.0.note).unwrap_or_default();
    state
        .services
        .decisions()
        .close_credit(id, admin.0.admin_id, &note)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "id": id })))
}

async fn pending_refunds(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<PendingQuery>,
) -> ApiResult<Json<Vec<RefundRequestView>>> {
    admin.require(Permission::Refunds)?;
    Ok(Json(
        state
            .services
            .decisions()
            .refund_requests(query.limit.unwrap_or(REFUND_LIST_LIMIT), query.stuck)
            .await?,
    ))
}

async fn refund_actions(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<LimitQuery>,
) -> ApiResult<Json<Vec<RefundActionView>>> {
    admin.require(Permission::Refunds)?;
    Ok(Json(
        state
            .services
            .decisions()
            .refund_actions(query.limit.unwrap_or(REFUND_LIST_LIMIT))
            .await?,
    ))
}

async fn cancel_refund_request(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<ReasonRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let reason = body.and_then(|b| b.0.reason).unwrap_or_default();
    state
        .services
        .inbox()
        .cancel_refund_request(id, admin.0.admin_id, &reason)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "request_id": id })))
}

async fn confirm_refund_request(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<NoteRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let note = body.and_then(|b| b.0.note).unwrap_or_default();
    let result = state
        .services
        .inbox()
        .confirm_refund_request(id, admin.0.admin_id, &note)
        .await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "request_id": id, "result": result }),
    ))
}

async fn retry_refund_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let outcome = state
        .services
        .inbox()
        .retry_refund_action(id, admin.0.admin_id)
        .await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "request_id": id, "outcome": outcome }),
    ))
}

async fn close_refund_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<NoteRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Refunds)?;
    let note = body.and_then(|b| b.0.note).unwrap_or_default();
    state
        .services
        .inbox()
        .close_refund_action(id, admin.0.admin_id, &note)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true, "request_id": id })))
}

async fn webhooks(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<WebhookListQuery>,
) -> ApiResult<Json<JournalPage>> {
    admin.require(Permission::Money)?;
    let filter = JournalQuery {
        provider: query.provider,
        outcome: query.outcome,
        from: query.from,
        to: query.to,
        q: query.q,
    };
    Ok(Json(
        state
            .services
            .inbox()
            .journal(&filter, PageRequest::new(query.limit, query.after))
            .await?,
    ))
}

async fn webhook(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<JournalCard>> {
    admin.require(Permission::Money)?;
    Ok(Json(state.services.inbox().journal_card(id).await?))
}

async fn retry_webhook(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<JournalCard>> {
    admin.require(Permission::Money)?;
    Ok(Json(
        state
            .services
            .inbox()
            .retry_webhook(id, admin.0.admin_id)
            .await?,
    ))
}
