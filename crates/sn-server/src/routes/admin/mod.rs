mod admins;
mod alerts;
mod api_keys;
mod audit;
mod auth;
mod backups;
mod bot_content;
mod cabinet;
mod catalog;
mod clients;
mod connect_apps;
#[cfg(test)]
mod decisions_tests;
mod delivery;
mod documents;
mod ext_modules;
mod import;
mod infra;
mod landing;
mod mail;
mod miniapp;
mod money;
mod overview;
pub(crate) mod photos;
mod referrals;
mod remna_link;
mod remna_sync;
#[cfg(test)]
mod review_tests;
mod settings;
mod stars;
mod support;
mod system;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::Router;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sn_domain::paging::PageRequest;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
        .merge(admins::routes())
        .merge(api_keys::routes())
        .merge(audit::routes())
        .merge(ext_modules::routes())
        .merge(mail::routes())
        .merge(alerts::routes())
        .merge(overview::routes())
        .merge(clients::routes())
        .merge(referrals::routes())
        .merge(photos::routes())
        .merge(money::routes())
        .merge(stars::routes())
        .merge(delivery::routes())
        .merge(catalog::routes())
        .merge(infra::routes())
        .merge(support::routes())
        .merge(settings::routes())
        .merge(landing::routes())
        .merge(miniapp::routes())
        .merge(cabinet::routes())
        .merge(connect_apps::routes())
        .merge(system::routes())
        .merge(backups::routes())
        .merge(bot_content::routes())
        .merge(documents::routes())
        .merge(import::routes())
        .merge(remna_sync::routes())
        .merge(remna_link::routes())
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct Paging {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub after: Option<i64>,
}

impl Paging {
    pub(crate) fn request(self) -> PageRequest {
        PageRequest::new(self.limit, self.after)
    }
}

pub(crate) fn decode<T: DeserializeOwned>(value: serde_json::Value) -> ApiResult<T> {
    serde_json::from_value(value)
        .map_err(|e| ApiError::bad_request("validation.invalid_body", e.to_string()))
}

pub(crate) fn json<T: serde::Serialize>(value: &T) -> ApiResult<serde_json::Value> {
    serde_json::to_value(value)
        .map_err(|e| ApiError::from(sn_services::ServiceError::Internal(e.to_string())))
}
