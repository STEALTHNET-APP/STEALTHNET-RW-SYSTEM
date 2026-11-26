use super::Paging;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::ClientId;
use sn_domain::paging::Page;
use sn_services::referral_links::{Candidate, Invited, ReferralSection, ReferrerChange};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clients/{id}/referrals", get(section))
        .route("/clients/{id}/referrals/invited", get(invited))
        .route("/clients/{id}/referrals/invited/{invitee}", delete(unlink))
        .route(
            "/clients/{id}/referrer",
            put(set_referrer).delete(remove_referrer),
        )
        .route("/clients/{id}/referrer/candidates", get(candidates))
}

#[derive(Deserialize)]
struct SectionQuery {
    #[serde(default)]
    limit: Option<u32>,
}

async fn section(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<SectionQuery>,
) -> ApiResult<Json<ReferralSection>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .referral_links()
            .section(ClientId(id), admin.can(Permission::Money), query.limit)
            .await?,
    ))
}

async fn invited(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(paging): ApiQuery<Paging>,
) -> ApiResult<Json<Page<Invited>>> {
    admin.require(Permission::ClientsRead)?;
    Ok(Json(
        state
            .services
            .referral_links()
            .invited(ClientId(id), paging.request(), admin.can(Permission::Money))
            .await?,
    ))
}

#[derive(Deserialize)]
struct CandidatesQuery {
    #[serde(default)]
    q: Option<String>,
}

async fn candidates(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<CandidatesQuery>,
) -> ApiResult<Json<Vec<Candidate>>> {
    admin.require(Permission::ClientsWrite)?;
    Ok(Json(
        state
            .services
            .referral_links()
            .candidates(ClientId(id), query.q.as_deref().unwrap_or(""))
            .await?,
    ))
}

#[derive(Deserialize)]
struct ReferrerRequest {
    referrer_id: i64,
}

async fn set_referrer(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<ReferrerRequest>,
) -> ApiResult<Json<ReferrerChange>> {
    admin.require(Permission::ClientsWrite)?;
    Ok(Json(
        state
            .services
            .referral_links()
            .set_referrer(ClientId(id), ClientId(body.referrer_id), admin.actor())
            .await?,
    ))
}

async fn remove_referrer(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<ReferrerChange>> {
    admin.require(Permission::ClientsWrite)?;
    Ok(Json(
        state
            .services
            .referral_links()
            .remove_referrer(ClientId(id), None, admin.actor())
            .await?,
    ))
}

async fn unlink(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path((id, invitee)): Path<(i64, i64)>,
) -> ApiResult<Json<ReferrerChange>> {
    admin.require(Permission::ClientsWrite)?;
    Ok(Json(
        state
            .services
            .referral_links()
            .remove_referrer(ClientId(invitee), Some(ClientId(id)), admin.actor())
            .await?,
    ))
}
