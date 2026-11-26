pub mod config;
pub mod cookies;
pub mod error;
pub mod extract;
pub mod lang;
pub mod maintenance;
pub mod routes;
pub mod security;
pub mod state;
pub mod web;

pub use config::ServerConfig;
pub use error::{ApiError, ApiResult};
pub use routes::router;
pub use state::AppState;

use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;

pub async fn serve(state: AppState, cancel: CancellationToken) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(state.config.bind).await?;
    tracing::info!(addr = %state.config.bind, "HTTP-сервер запущен");
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { cancel.cancelled().await })
    .await
}

#[cfg(test)]
mod tests;
