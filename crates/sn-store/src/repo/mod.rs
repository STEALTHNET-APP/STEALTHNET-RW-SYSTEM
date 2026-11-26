mod admins;
mod api_keys;
mod audit;
mod audit_journal;
mod billing;
mod bot_images;
mod client_filter;
mod client_history;
mod client_photos;
mod client_search;
mod clients;
mod document_pages;
mod events;
mod jobs;
mod ledger;
mod modules;
mod notices;
mod orders;
mod payments;
mod promotions;
mod remna_events;
mod secrets;
mod sessions;
mod settings;
mod site_assets;
mod subscriptions;
mod tariffs;
mod ticket_files;
mod tickets;
mod trials;
mod webhooks;

pub use admins::*;
pub use api_keys::*;
pub use audit::*;
pub use audit_journal::*;
pub use billing::*;
pub use bot_images::*;
pub use client_filter::*;
pub use client_history::*;
pub use client_photos::*;
pub use client_search::*;
pub use clients::*;
pub use document_pages::*;
pub use events::*;
pub use jobs::*;
pub use ledger::*;
pub use modules::*;
pub use notices::*;
pub use orders::*;
pub use payments::*;
pub use promotions::*;
pub use remna_events::*;
pub use secrets::*;
pub use sessions::*;
pub use settings::*;
pub use site_assets::*;
pub use subscriptions::*;
pub use tariffs::*;
pub use ticket_files::{FileOwner, NewTicketFile, StoredFile, TicketFilesRepo};
pub use tickets::*;
pub use trials::*;
pub use webhooks::*;

use crate::error::{StoreError, StoreResult};

pub(crate) fn non_negative_u32(value: i32, field: &'static str) -> StoreResult<u32> {
    u32::try_from(value).map_err(|_| StoreError::Decode(format!("{field}: отрицательное значение")))
}

pub(crate) fn non_negative_u64(value: i64, field: &'static str) -> StoreResult<u64> {
    u64::try_from(value).map_err(|_| StoreError::Decode(format!("{field}: отрицательное значение")))
}

pub(crate) fn to_i32(value: u32, field: &'static str) -> StoreResult<i32> {
    i32::try_from(value)
        .map_err(|_| StoreError::Invalid(format!("{field}: слишком большое значение")))
}

pub(crate) fn to_i64(value: u64, field: &'static str) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| StoreError::Invalid(format!("{field}: слишком большое значение")))
}
