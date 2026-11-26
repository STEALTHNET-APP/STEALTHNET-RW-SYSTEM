use super::{
    AdminGift, Counts, GiftError, GiftOffer, ListFilter, MyGift, RedeemPreview, Redeemed,
    SettingsUpdate, SettingsView,
};
use crate::catalog::installation_currency;
use crate::catalog::web::{Admin, Customer, WebError, drop_fields, json_value};
use crate::context::{GrantTarget, ModuleContext};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sn_domain::access::Permission;
use sn_domain::ids::SubscriptionId;
use sn_domain::paging::{Page, PageRequest};

impl From<GiftError> for WebError {
    fn from(err: GiftError) -> Self {
        match err {
            GiftError::Rejected(rejection) => {
                WebError::rejected(rejection.code(), rejection.message())
            }
            GiftError::Module(err) => err.into(),
        }
    }
}

pub fn admin(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/gifts", get(list))
        .route("/summary", get(summary))
        .route("/settings", get(settings).put(save_settings))
        .with_state(ctx)
}

pub fn client(ctx: ModuleContext) -> Router {
    Router::new()
        .route("/offers", get(offers))
        .route("/mine", get(mine))
        .route("/preview", post(preview))
        .route("/redeem", post(redeem))
        .with_state(ctx)
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    after: Option<i64>,
}

async fn list(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, WebError> {
    admin.require(Permission::Marketing)?;
    let filter = ListFilter::parse(query.status.as_deref()).ok_or_else(|| {
        WebError::rejected(
            "validation.invalid_value",
            "фильтр: all, active, redeemed, expired или canceled",
        )
    })?;
    let page: Page<AdminGift> = super::admin_list(
        &ctx,
        filter,
        query.q.as_deref(),
        PageRequest::new(query.limit, query.after),
    )
    .await?;
    let mut value = json_value(&page)?;
    if !admin.sees_money() {
        drop_fields(value.get_mut("items"), &["paid"]);
    }
    Ok(Json(value))
}

async fn summary(State(ctx): State<ModuleContext>, admin: Admin) -> Result<Json<Counts>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::counts(&ctx).await?))
}

async fn settings(
    State(ctx): State<ModuleContext>,
    admin: Admin,
) -> Result<Json<SettingsView>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(super::settings_view(&ctx).await?))
}

async fn save_settings(
    State(ctx): State<ModuleContext>,
    admin: Admin,
    Json(body): Json<SettingsUpdate>,
) -> Result<Json<SettingsView>, WebError> {
    admin.require(Permission::Marketing)?;
    Ok(Json(
        super::save_settings(&ctx, &body, Some(admin.0.admin_id)).await?,
    ))
}

#[derive(Serialize)]
struct OffersView {
    offers: Vec<GiftOffer>,
    code_days: u32,
}

async fn offers(
    State(ctx): State<ModuleContext>,
    _customer: Customer,
) -> Result<Json<OffersView>, WebError> {
    let mut conn = ctx.store.conn().await?;
    let currency = installation_currency(&mut conn).await?;
    let settings = super::settings(&mut conn).await?;
    Ok(Json(OffersView {
        offers: super::offers(
            &mut conn,
            currency,
            ctx.core.now(),
            ctx.core.core_settings().remna_offset(),
        )
        .await?,
        code_days: settings.code_days,
    }))
}

async fn mine(
    State(ctx): State<ModuleContext>,
    customer: Customer,
) -> Result<Json<Vec<MyGift>>, WebError> {
    Ok(Json(super::mine(&ctx, customer.0.client_id).await?))
}

#[derive(Deserialize)]
struct CodeRequest {
    code: String,
    #[serde(default)]
    subscription_id: Option<SubscriptionId>,
    #[serde(default)]
    new: bool,
}

impl CodeRequest {
    fn target(&self) -> GrantTarget {
        match (self.subscription_id, self.new) {
            (Some(id), _) => GrantTarget::Subscription(id),
            (None, true) => GrantTarget::New,
            (None, false) => GrantTarget::Auto,
        }
    }
}

async fn preview(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(body): Json<CodeRequest>,
) -> Result<Json<RedeemPreview>, WebError> {
    Ok(Json(
        super::preview(&ctx, customer.0.client_id, &body.code, body.target()).await?,
    ))
}

async fn redeem(
    State(ctx): State<ModuleContext>,
    customer: Customer,
    Json(body): Json<CodeRequest>,
) -> Result<Json<Redeemed>, WebError> {
    customer.require_full()?;
    let ctx_for_task = ctx.clone();
    let client_id = customer.0.client_id;
    let target = body.target();
    let code = body.code;
    let result =
        tokio::spawn(async move { super::redeem(&ctx_for_task, client_id, &code, target).await })
            .await
            .map_err(|_| {
                WebError::from(crate::error::ModuleError::Core(
                    "активация прервалась".into(),
                ))
            })?;
    Ok(Json(result?))
}
