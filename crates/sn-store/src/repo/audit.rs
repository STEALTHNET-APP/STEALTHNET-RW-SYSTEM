use crate::error::StoreResult;
use chrono::{DateTime, Utc};
use sqlx::PgConnection;

pub mod audit_entity {
    pub const CLIENT: &str = "client";
    pub const SUBSCRIPTION: &str = "subscription";
    pub const ORDER: &str = "order";
    pub const PAYMENT: &str = "payment";
    pub const TICKET: &str = "ticket";
    pub const TARIFF: &str = "tariff";
    pub const ADMIN: &str = "admin";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorKind {
    Admin,
    Client,
    System,
    Bot,
}

impl ActorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ActorKind::Admin => "admin",
            ActorKind::Client => "client",
            ActorKind::System => "system",
            ActorKind::Bot => "bot",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub actor_kind: ActorKind,
    pub actor_id: Option<i64>,
    pub action: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<i64>,
    pub details: serde_json::Value,
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow, serde::Serialize)]
pub struct AuditRecord {
    pub id: i64,
    pub actor_kind: String,
    pub actor_id: Option<i64>,
    pub action: String,
    pub entity_kind: Option<String>,
    pub entity_id: Option<i64>,
    pub details: serde_json::Value,
    pub created_at: DateTime<Utc>,
    #[sqlx(default)]
    pub ext_module: Option<String>,
}

const COLUMNS: &str =
    "id, actor_kind, actor_id, action, entity_kind, entity_id, details, created_at, ext_module";

tokio::task_local! {
    static VIA_API_KEY: i64;
}

pub async fn with_api_key<F: std::future::Future>(key: Option<i64>, future: F) -> F::Output {
    match key {
        Some(key) => VIA_API_KEY.scope(key, future).await,
        None => future.await,
    }
}

pub fn current_api_key() -> Option<i64> {
    VIA_API_KEY.try_with(|key| *key).ok()
}

pub struct AuditRepo<'c> {
    conn: &'c mut PgConnection,
}

impl<'c> AuditRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        AuditRepo { conn }
    }

    pub async fn record(&mut self, entry: &AuditEntry) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "INSERT INTO audit_log (actor_kind, actor_id, action, entity_kind, entity_id, details, ip, \
             api_key_id, ext_module) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
             (SELECT m.id FROM ext_modules m WHERE m.api_key_id = $8)) RETURNING id",
        )
        .bind(entry.actor_kind.as_str())
        .bind(entry.actor_id)
        .bind(&entry.action)
        .bind(&entry.entity_kind)
        .bind(entry.entity_id)
        .bind(&entry.details)
        .bind(&entry.ip)
        .bind(current_api_key())
        .fetch_one(&mut *self.conn)
        .await?)
    }

    pub async fn for_entity(
        &mut self,
        entity_kind: &str,
        entity_id: i64,
        limit: i64,
    ) -> StoreResult<Vec<AuditRecord>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM audit_log WHERE entity_kind = $1 AND entity_id = $2 ORDER BY id DESC LIMIT $3"
        ))
        .bind(entity_kind)
        .bind(entity_id)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn recent_by_action(
        &mut self,
        action: &str,
        limit: i64,
    ) -> StoreResult<Vec<AuditRecord>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM audit_log WHERE action = $1 ORDER BY id DESC LIMIT $2"
        ))
        .bind(action)
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await?)
    }

    pub async fn by_ids(&mut self, ids: &[i64]) -> StoreResult<Vec<AuditRecord>> {
        Ok(sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM audit_log WHERE id = ANY($1) ORDER BY id"
        ))
        .bind(ids)
        .fetch_all(&mut *self.conn)
        .await?)
    }
}
