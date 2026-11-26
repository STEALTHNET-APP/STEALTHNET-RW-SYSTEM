use super::Paging;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{TicketFileId, TicketId};
use sn_domain::support::{TicketFilter, TicketOrder};
use sn_services::support::{CloseMany, ReplyRequest, TicketThread};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/support/tickets", get(list))
        .route(
            "/support/tickets/close-many",
            get(close_many_count).post(close_many),
        )
        .route(
            "/support/tickets/{id}",
            get(thread).post(reply).delete(remove),
        )
        .route("/support/tickets/{id}/close", post(close))
        .route("/support/tickets/{id}/reopen", post(reopen))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    after_id: Option<i64>,
    #[serde(default)]
    order: Option<String>,
}

fn filter(value: Option<&str>) -> ApiResult<TicketFilter> {
    match value.map(str::trim).unwrap_or("waiting") {
        "" | "waiting" => Ok(TicketFilter::Waiting),
        "answered" => Ok(TicketFilter::Answered),
        "closed" => Ok(TicketFilter::Closed),
        other => Err(ApiError::bad_request(
            "validation.filter",
            format!("неизвестный фильтр {other}"),
        )),
    }
}

fn order(value: Option<&str>) -> ApiResult<TicketOrder> {
    match value.map(str::trim).unwrap_or("recent") {
        "" | "recent" => Ok(TicketOrder::Recent),
        "longest" => Ok(TicketOrder::Longest),
        other => Err(ApiError::bad_request(
            "validation.order",
            format!("неизвестный порядок {other}"),
        )),
    }
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    let filter = filter(query.filter.as_deref())?;
    let order = order(query.order.as_deref())?;
    let paging = Paging {
        limit: query.limit,
        after: query.after,
    };
    let (page, counts) = state
        .services
        .support()
        .list_view(
            filter,
            order,
            paging.request(),
            query.after_id.map(TicketId),
            query.q.as_deref(),
        )
        .await?;
    let next_id = page
        .next
        .and_then(|_| page.items.last().map(|item| item.ticket.id));
    Ok(Json(serde_json::json!({
        "items": page.items,
        "next": page.next,
        "next_id": next_id,
        "counts": counts,
        "files": state.services.support().file_limits(),
    })))
}

async fn thread(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TicketThread>> {
    admin.require(Permission::Support)?;
    Ok(Json(
        state
            .services
            .support()
            .thread(TicketId(id), admin.actor())
            .await?,
    ))
}

#[derive(Deserialize)]
struct ReplyBody {
    #[serde(default)]
    body: String,
    #[serde(default)]
    close: bool,
    #[serde(default)]
    files: Vec<TicketFileId>,
}

async fn reply(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<ReplyBody>,
) -> ApiResult<Json<TicketThread>> {
    admin.require(Permission::Support)?;
    let support = state.services.support();
    support
        .reply_with_files(
            TicketId(id),
            &ReplyRequest {
                body: body.body,
                close: body.close,
            },
            &body.files,
            admin.actor(),
        )
        .await?;
    Ok(Json(support.thread(TicketId(id), admin.actor()).await?))
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    state
        .services
        .support()
        .delete(TicketId(id), admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn close(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    state
        .services
        .support()
        .close(TicketId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reopen(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    state
        .services
        .support()
        .reopen(TicketId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct WaitingQuery {
    waiting_days: u32,
}

async fn close_many_count(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<WaitingQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    let count = state
        .services
        .support()
        .count_waiting_longer(query.waiting_days)
        .await?;
    Ok(Json(serde_json::json!({ "count": count })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseManyBody {
    #[serde(default)]
    ids: Option<Vec<i64>>,
    #[serde(default)]
    waiting_days: Option<u32>,
}

async fn close_many(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CloseManyBody>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Support)?;
    let request = match (body.ids, body.waiting_days) {
        (Some(ids), None) => CloseMany::Tickets(ids.into_iter().map(TicketId).collect()),
        (None, Some(days)) => CloseMany::WaitingDays(days),
        _ => {
            return Err(ApiError::bad_request(
                "validation.close_many",
                "укажите ids или waiting_days",
            ));
        }
    };
    let closed = state
        .services
        .support()
        .close_many(&request, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "closed": closed })))
}
