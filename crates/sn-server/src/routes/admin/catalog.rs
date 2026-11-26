use super::json;
use crate::error::ApiResult;
use crate::extract::{ApiJson, ApiQuery, CurrentAdmin};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use sn_domain::access::Permission;
use sn_domain::ids::{CategoryId, TariffId};
use sn_domain::tariff::{Category, Tariff, TariffChannels};
use sn_services::promotions::{PromotionInput, PromotionView, PromotionsView};
use sn_services::tariffs::{CategoryInput, SquadBrief, TariffInput, TariffsView};
use sn_store::TariffUsage;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tariffs", get(list).post(create))
        .route("/tariffs/squads", get(squads))
        .route(
            "/tariffs/promotions",
            get(promotions).post(create_promotion),
        )
        .route(
            "/tariffs/promotions/{id}",
            put(update_promotion)
                .patch(switch_promotion)
                .delete(remove_promotion),
        )
        .route(
            "/tariffs/{id}",
            get(one).put(update).patch(visibility).delete(remove),
        )
        .route("/tariffs/{id}/channels", put(channels))
        .route("/tariffs/{id}/usage", get(usage))
        .route("/categories", get(categories).post(create_category))
        .route(
            "/categories/{id}",
            put(update_category).delete(remove_category),
        )
}

async fn list(admin: CurrentAdmin, State(state): State<AppState>) -> ApiResult<Json<TariffsView>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(state.services.tariffs().list().await?))
}

async fn promotions(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<PromotionsView>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(state.services.promotions().list().await?))
}

async fn create_promotion(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<PromotionInput>,
) -> ApiResult<Json<PromotionView>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .promotions()
            .create(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_promotion(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<PromotionInput>,
) -> ApiResult<Json<PromotionView>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .promotions()
            .update(id, &body, admin.0.admin_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct PromotionSwitch {
    enabled: bool,
}

async fn switch_promotion(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<PromotionSwitch>,
) -> ApiResult<Json<PromotionView>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .promotions()
            .set_enabled(id, body.enabled, admin.0.admin_id)
            .await?,
    ))
}

async fn remove_promotion(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Tariffs)?;
    state
        .services
        .promotions()
        .delete(id, admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn one(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(state.services.tariffs().get(TariffId(id)).await?))
}

async fn usage(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TariffUsage>> {
    admin.require(Permission::Tariffs)?;
    let people = admin.can(Permission::ClientsRead);
    Ok(Json(
        state.services.tariffs().usage(TariffId(id), people).await?,
    ))
}

async fn create(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<TariffInput>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .create(&body, admin.0.admin_id)
            .await?,
    ))
}

#[derive(Deserialize, Default)]
struct UpdateQuery {
    #[serde(default)]
    apply: Option<bool>,
}

async fn update(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiQuery(query): ApiQuery<UpdateQuery>,
    ApiJson(body): ApiJson<TariffInput>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Tariffs)?;
    let (tariff, applied) = state
        .services
        .tariffs()
        .update_and_apply(
            TariffId(id),
            &body,
            admin.0.admin_id,
            query.apply.unwrap_or(false),
        )
        .await?;
    let mut value = json(&tariff)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("applied".into(), json(&applied)?);
    }
    Ok(Json(value))
}

async fn remove(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Tariffs)?;
    state
        .services
        .tariffs()
        .delete(TariffId(id), admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn channels(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<TariffChannels>,
) -> ApiResult<Json<TariffChannels>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .set_channels(TariffId(id), body, admin.0.admin_id)
            .await?,
    ))
}

async fn squads(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SquadBrief>>> {
    if !admin.can(Permission::Marketing) {
        admin.require(Permission::Tariffs)?;
    }
    Ok(Json(state.services.tariffs().squads().await?))
}

async fn remove_category(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Tariffs)?;
    state
        .services
        .tariffs()
        .delete_category(CategoryId(id), admin.0.admin_id)
        .await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct VisibilityRequest {
    on_sale: bool,
}

async fn visibility(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<VisibilityRequest>,
) -> ApiResult<Json<Tariff>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .set_on_sale(TariffId(id), body.on_sale, admin.0.admin_id)
            .await?,
    ))
}

async fn categories(
    admin: CurrentAdmin,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    admin.require(Permission::Tariffs)?;
    let view = state.services.tariffs().list().await?;
    Ok(Json(serde_json::json!({ "categories": view.categories })))
}

async fn create_category(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CategoryInput>,
) -> ApiResult<Json<Category>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .create_category(&body, admin.0.admin_id)
            .await?,
    ))
}

async fn update_category(
    admin: CurrentAdmin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<CategoryInput>,
) -> ApiResult<Json<Category>> {
    admin.require(Permission::Tariffs)?;
    Ok(Json(
        state
            .services
            .tariffs()
            .update_category(CategoryId(id), &body, admin.0.admin_id)
            .await?,
    ))
}
