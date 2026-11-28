use super::{Paging, json};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_domain::money::Money;
use sn_domain::subscription::admin_status;
use sn_services::clients::HistoryPage;
use sn_services::email_admin::{EMAIL_TAKEN, EmailRemoved, EmailSet};
use sn_store::{ClientConditions, ClientFilter, ClientListItem};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clients", get(list))
        .route("/clients/segments", get(segments))
        .route("/clients/filter-options", get(filter_options))
        .route("/clients/grant-options", get(grant_options))
        .route("/clients/{id}", get(card))
        .route("/clients/{id}/history", get(history))
        .route("/clients/{id}/module-cards", get(module_cards))
        .route("/clients/{id}/note", put(note))
        .route("/clients/{id}/balance", post(balance))
        .route("/clients/{id}/message", post(message))
        .route("/clients/{id}/grant", post(grant))
        .route("/clients/{id}/delete", post(delete_client))
        .route("/clients/{id}/block", post(block))
        .route("/clients/{id}/unblock", post(unblock))
        .route("/clients/{id}/totp/reset", post(reset_two_factor))
        .route("/clients/{id}/password", post(issue_password))
        .route("/clients/{id}/email", put(set_email).delete(remove_email))
        .route("/subscriptions/{id}", get(subscription))
        .route(
            "/subscriptions/{id}/actions/{action}",
            post(subscription_action),
        )
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

fn client_filter(
    admin: &CurrentAdmin,
    pairs: &[(String, String)],
    query: Option<&str>,
) -> ApiResult<ClientFilter> {
    let conditions = ClientConditions::from_query(pairs.iter().map(|(k, v)| (k, v)))
        .map_err(|message| ApiError::bad_request("validation.filter", message))?;
    if conditions.uses_money() && !admin.can(Permission::Money) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "Условия про оплаты и баланс доступны с правом «Деньги»",
        ));
    }
    Ok(ClientFilter {
        conditions,
        query: query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| q.chars().take(200).collect()),
    })
}

fn list_item(
    item: &ClientListItem,
    state: &AppState,
    money: bool,
    tariffs: &std::collections::HashMap<sn_domain::ids::TariffId, String>,
) -> serde_json::Value {
    let currency = state.services.settings().currency;
    let tariff_name = item
        .subscription
        .as_ref()
        .and_then(|s| s.tariff_id)
        .and_then(|id| tariffs.get(&id).cloned());
    serde_json::json!({
        "tariff_name": tariff_name,
        "client": item.client,
        "name": item.name,
        "telegram_id": item.telegram_id,
        "telegram_username": item.telegram_username,
        "email": item.email,
        "status": admin_status(item.subscription.as_ref(), item.client.is_blocked(), state.services.now()),
        "subscription": item.subscription,
        "balance": money.then(|| Money::new(item.balance_minor, currency)),
        "paid_total": money.then(|| Money::new(item.paid_total_minor, currency)),
    })
}

async fn list(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(pairs): ApiQuery<Vec<(String, String)>>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsRead)?;
    let filter = client_filter(&admin, &pairs, query.q.as_deref())?;
    let paging = Paging {
        limit: query.limit,
        after: query.after,
    };
    let page = state
        .services
        .clients()
        .search(&filter, paging.request())
        .await?;
    let money = admin.can(Permission::Money);
    let tariffs = state.services.clients().tariff_names().await?;
    let items: Vec<serde_json::Value> = page
        .items
        .iter()
        .map(|item| list_item(item, &state, money, &tariffs))
        .collect();
    Ok(Json(
        serde_json::json!({ "items": items, "next": page.next }),
    ))
}

async fn segments(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiQuery(pairs): ApiQuery<Vec<(String, String)>>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> ApiResult<Json<sn_store::SegmentCounts>> {
    admin.require(Permission::ClientsRead)?;
    let filter = client_filter(&admin, &pairs, query.q.as_deref())?;
    let clients = state.services.clients();
    Ok(Json(if filter == ClientFilter::default() {
        clients.segment_counts().await?
    } else {
        clients.segment_counts_where(&filter).await?
    }))
}

async fn filter_options(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<sn_store::FilterOptions>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(state.services.clients().filter_options().await?))
}

async fn card(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsRead)?;
    let card = state.services.clients().card(ClientId(id)).await?;
    let mut value = json(&card)?;
    if !admin.can(Permission::Money)
        && let Some(object) = value.as_object_mut()
    {
        object.remove("balance");
        object.remove("balance_held");
        object.remove("referral_balance");
        if let Some(stats) = object.get_mut("stats").and_then(|s| s.as_object_mut()) {
            stats.remove("paid_total");
            stats.remove("payments_count");
        }
    }
    Ok(Json(value))
}

async fn history(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(paging): ApiQuery<Paging>,
) -> ApiResult<Json<HistoryPage>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .clients()
            .history(ClientId(id), paging.request(), admin.can(Permission::Money))
            .await?,
    ))
}

#[derive(Deserialize)]
struct NoteRequest {
    note: Option<String>,
}

async fn note(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<NoteRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    state
        .services
        .clients()
        .set_note(ClientId(id), body.note.as_deref(), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn grant_options(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    let tariffs = state.services.clients().grant_options().await?;
    Ok(Json(serde_json::json!({
        "tariffs": tariffs,
        "multi_subscriptions": state.services.settings().multi_subscriptions,
    })))
}

async fn module_cards(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<sn_services::ext_modules::cards::CardFrame>>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .ext_modules()
            .client_cards(&admin.0, ClientId(id))
            .await?,
    ))
}

async fn grant(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<sn_services::clients::GrantRequest>,
) -> ApiResult<Json<sn_services::clients::GrantResult>> {
    admin.require(Permission::ClientsWrite)?;
    Ok(Json(
        state
            .services
            .clients()
            .grant(ClientId(id), &body, admin.actor())
            .await?,
    ))
}

async fn delete_client(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    admin.require(Permission::Money)?;
    state
        .services
        .clients()
        .delete(ClientId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MessageRequest {
    text: String,
}

async fn message(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<MessageRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    state
        .services
        .clients()
        .send_message(ClientId(id), &body.text, admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct BalanceRequest {
    delta: Money,
    comment: String,
    idempotency_key: String,
}

async fn balance(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<BalanceRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Money)?;
    if body.comment.trim().is_empty() || body.idempotency_key.trim().is_empty() {
        return Err(ApiError::bad_request(
            "validation.balance",
            "укажите причину изменения баланса",
        ));
    }
    let balance = state
        .services
        .clients()
        .adjust_balance(
            ClientId(id),
            body.delta,
            body.comment.trim(),
            admin.0.admin_id,
            body.idempotency_key.trim(),
        )
        .await?;
    Ok(Json(serde_json::json!({ "balance": balance })))
}

#[derive(Deserialize, Default)]
struct BlockRequest {
    #[serde(default)]
    reason: Option<String>,
}

async fn block(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<ApiJson<BlockRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    let body = body.map(|b| b.0).unwrap_or_default();
    state
        .services
        .clients()
        .block(ClientId(id), body.reason.as_deref(), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn unblock(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsWrite)?;
    state
        .services
        .clients()
        .unblock(ClientId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn reset_two_factor(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsLogin)?;
    state
        .services
        .clients()
        .reset_two_factor(ClientId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn issue_password(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::ClientsLogin)?;
    admin.require_session()?;
    let password = state
        .services
        .auth()
        .issue_client_password(ClientId(id), admin.actor())
        .await?;
    Ok(Json(serde_json::json!({ "password": password })))
}

#[derive(Deserialize)]
struct EmailRequest {
    email: String,
}

async fn set_email(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<EmailRequest>,
) -> ApiResult<Response> {
    admin.require(Permission::ClientsLogin)?;
    admin.require_session()?;
    let result = state
        .services
        .clients()
        .set_email(ClientId(id), &body.email, admin.actor())
        .await?;
    Ok(match result {
        EmailSet::Taken { client_id } => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": {
                    "code": EMAIL_TAKEN,
                    "message": "Эта почта уже указана у другого клиента",
                    "client_id": client_id,
                }
            })),
        )
            .into_response(),
        saved @ EmailSet::Saved { .. } => Json(saved).into_response(),
    })
}

async fn remove_email(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<EmailRemoved>> {
    admin.require(Permission::ClientsLogin)?;
    Ok(Json(
        state
            .services
            .clients()
            .remove_email(ClientId(id), admin.actor())
            .await?,
    ))
}

async fn subscription(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<sn_services::subscriptions::AdminSubscriptionView>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .subscriptions()
            .admin_view(SubscriptionId(id), admin.actor())
            .await?,
    ))
}

#[derive(Deserialize, Default)]
struct ActionRequest {
    #[serde(default)]
    days: Option<u32>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    hwid: Option<String>,
    #[serde(default)]
    limits: Option<sn_services::subscriptions::LimitsChange>,
}

async fn subscription_action(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((id, action)): Path<(i64, String)>,
    body: Option<ApiJson<ActionRequest>>,
) -> ApiResult<Json<serde_json::Value>> {
    let reading = matches!(action.as_str(), "devices" | "connections" | "requests");
    admin.require(if reading {
        Permission::ClientsRead
    } else {
        Permission::ClientsWrite
    })?;
    if action == "delete" {
        admin.require(Permission::Money)?;
    }
    let body = body.map(|b| b.0).unwrap_or_default();
    let ops = state.services.subscriptions();
    let actor = admin.actor();
    let id = SubscriptionId(id);
    let result = match action.as_str() {
        "extend" => {
            let days = body.days.unwrap_or(7);
            if !(1..=3650).contains(&days) {
                return Err(ApiError::bad_request(
                    "validation.days",
                    "продлить можно на 1–3650 дней",
                ));
            }
            json(&ops.add_days(id, days, actor).await?)
        }
        "undo-extend" => json(
            &ops.undo_add_days(id, body.count.unwrap_or(1), actor)
                .await?,
        ),
        "delete" => {
            ops.end_by_admin(id, actor).await?;
            Ok(serde_json::json!({ "ok": true }))
        }
        "connections" => json(&ops.connections(id, body.job_id.as_deref(), actor).await?),
        "requests" => json(&ops.subscription_requests(id, actor).await?),
        "reset-traffic" => json(&ops.reset_traffic(id, actor).await?),
        "reissue-link" => json(&ops.reissue_link(id, actor).await?),
        "disconnect-devices" => json(&ops.disconnect_all_devices(id, actor).await?),
        "disconnect-device" => {
            let hwid = body
                .hwid
                .filter(|h| !h.trim().is_empty())
                .ok_or_else(|| ApiError::bad_request("validation.hwid", "не указано устройство"))?;
            json(&ops.disconnect_device(id, &hwid, actor).await?)
        }
        "devices" => json(&ops.devices(id, actor).await?),
        "renewal-cancel" => {
            ops.view(id, actor, false).await?;
            json(
                &state
                    .services
                    .renewals()
                    .cancel_subscription(id, sn_services::renewals::CancelSource::Operator, actor)
                    .await?,
            )
        }
        "renewal-check" => {
            ops.view(id, actor, false).await?;
            json(&state.services.renewals().check_subscription(id).await?)
        }
        "limits" => {
            let change = body.limits.ok_or_else(|| {
                ApiError::bad_request("validation.limits", "не указаны новые лимиты")
            })?;
            json(&ops.change_limits(id, &change, actor).await?)
        }
        _ => return Err(ApiError::not_found()),
    };
    result.map(Json)
}
