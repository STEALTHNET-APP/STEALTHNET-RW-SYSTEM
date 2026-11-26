use crate::crypto::SealedSecret;
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Utc};
use sn_domain::client::{
    Client, ClientStatus, Identity, IdentityKind, Locale, NewIdentity, RegistrationSource,
};
use sn_domain::ids::ClientId;
use sn_domain::paging::{Page, PageRequest};
use sn_domain::subscription::Subscription;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct TotpRow {
    totp_nonce: Option<Vec<u8>>,
    totp_secret: Option<Vec<u8>>,
    totp_enabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTotp {
    pub secret: SealedSecret,
    pub enabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClient {
    pub locale: Locale,
    pub registration_source: RegistrationSource,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientSegment {
    Active,
    Trial,
    NoSubscription,
    ExpiringSoon,
    NeverConnected,
    Expired,
    Limited,
    Blocked,
}

impl ClientSegment {
    pub const ALL: [ClientSegment; 8] = [
        ClientSegment::Active,
        ClientSegment::ExpiringSoon,
        ClientSegment::Trial,
        ClientSegment::NeverConnected,
        ClientSegment::Expired,
        ClientSegment::Limited,
        ClientSegment::Blocked,
        ClientSegment::NoSubscription,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ClientSegment::Active => "active",
            ClientSegment::Trial => "trial",
            ClientSegment::NoSubscription => "no_subscription",
            ClientSegment::ExpiringSoon => "expiring_soon",
            ClientSegment::NeverConnected => "never_connected",
            ClientSegment::Expired => "expired",
            ClientSegment::Limited => "limited",
            ClientSegment::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> StoreResult<Self> {
        ClientSegment::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or_else(|| StoreError::Invalid(format!("неизвестный сегмент клиентов: {value}")))
    }

    pub const fn title(self) -> &'static str {
        match self {
            ClientSegment::Active => "подписка действует",
            ClientSegment::Trial => "пробный период",
            ClientSegment::NoSubscription => "нет подписки",
            ClientSegment::ExpiringSoon => "подписка истекает за 3 дня",
            ClientSegment::NeverConnected => "не подключились",
            ClientSegment::Expired => "подписка истекла",
            ClientSegment::Limited => "трафик исчерпан",
            ClientSegment::Blocked => "заблокированы",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct ClientFilter {
    #[serde(flatten)]
    pub conditions: super::client_filter::ClientConditions,
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClientListItem {
    pub client: Client,
    pub name: Option<String>,
    pub telegram_id: Option<String>,
    pub telegram_username: Option<String>,
    pub email: Option<String>,
    pub subscription: Option<Subscription>,
    pub balance_minor: i64,
    pub paid_total_minor: i64,
}

#[derive(sqlx::FromRow)]
pub(super) struct ClientRow {
    id: i64,
    public_id: Uuid,
    status: String,
    block_reason: Option<String>,
    locale: String,
    note: Option<String>,
    registration_source: String,
    bot_unreachable: bool,
    created_at: DateTime<Utc>,
    blocked_at: Option<DateTime<Utc>>,
}

pub(super) const COLUMNS: &str = "c.id, c.public_id, c.status, c.block_reason, c.locale, c.note, c.registration_source, \
                       c.bot_unreachable, c.created_at, c.blocked_at";

impl TryFrom<ClientRow> for Client {
    type Error = StoreError;

    fn try_from(row: ClientRow) -> Result<Self, Self::Error> {
        Ok(Client {
            id: ClientId(row.id),
            public_id: row.public_id,
            status: ClientStatus::parse(&row.status)?,
            block_reason: row.block_reason,
            locale: Locale::parse(&row.locale)?,
            note: row.note,
            registration_source: RegistrationSource::parse(&row.registration_source)?,
            bot_unreachable: row.bot_unreachable,
            created_at: row.created_at,
            blocked_at: row.blocked_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct IdentityRow {
    kind: String,
    value: String,
    verified: bool,
    meta: serde_json::Value,
    created_at: DateTime<Utc>,
}

impl TryFrom<IdentityRow> for Identity {
    type Error = StoreError;

    fn try_from(row: IdentityRow) -> Result<Self, Self::Error> {
        Ok(Identity {
            kind: IdentityKind::parse(&row.kind)?,
            value: row.value,
            verified: row.verified,
            meta: row.meta,
            created_at: row.created_at,
        })
    }
}

pub(super) fn display_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let name = [first, last]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!name.is_empty()).then_some(name)
}

pub(super) fn fold_case(value: &str) -> String {
    value.to_lowercase().replace('ё', "е")
}

fn search_text(value: &str, meta: &serde_json::Value) -> String {
    let parts = ["username", "first_name", "last_name"]
        .into_iter()
        .filter_map(|key| meta.get(key).and_then(serde_json::Value::as_str));
    fold_case(
        &std::iter::once(value)
            .chain(parts)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub struct ClientsRepo<'c> {
    pub(super) conn: &'c mut PgConnection,
}

impl<'c> ClientsRepo<'c> {
    pub fn new(conn: &'c mut PgConnection) -> Self {
        ClientsRepo { conn }
    }

    pub async fn create(&mut self, client: &NewClient) -> StoreResult<Client> {
        let row: ClientRow = sqlx::query_as(&format!(
            "INSERT INTO clients AS c (locale, registration_source, note) VALUES ($1, $2, $3) RETURNING {COLUMNS}"
        ))
        .bind(client.locale.as_str())
        .bind(client.registration_source.as_str())
        .bind(&client.note)
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    async fn one(&mut self, condition: &str, bind: i64) -> StoreResult<Client> {
        let row: ClientRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM clients c WHERE {condition} AND c.deleted_at IS NULL"
        ))
        .bind(bind)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("клиент"))?;
        row.try_into()
    }

    pub async fn get(&mut self, id: ClientId) -> StoreResult<Client> {
        self.one("c.id = $1", id.0).await
    }

    pub async fn lock(&mut self, id: ClientId) -> StoreResult<Client> {
        let row: ClientRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM clients c WHERE c.id = $1 AND c.deleted_at IS NULL FOR UPDATE"
        ))
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("клиент"))?;
        row.try_into()
    }

    pub async fn by_public_id(&mut self, public_id: Uuid) -> StoreResult<Client> {
        let row: ClientRow = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM clients c WHERE c.public_id = $1 AND c.deleted_at IS NULL"
        ))
        .bind(public_id)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("клиент"))?;
        row.try_into()
    }

    pub async fn find_by_identity(
        &mut self,
        kind: IdentityKind,
        value: &str,
    ) -> StoreResult<Option<Client>> {
        let row: Option<ClientRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM clients c JOIN client_identities i ON i.client_id = c.id \
             WHERE i.kind = $1 AND i.value = $2 AND c.deleted_at IS NULL"
        ))
        .bind(kind.as_str())
        .bind(value)
        .fetch_optional(&mut *self.conn)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn add_identity(
        &mut self,
        client_id: ClientId,
        identity: &NewIdentity,
    ) -> StoreResult<Identity> {
        let row: IdentityRow = sqlx::query_as(
            "INSERT INTO client_identities (client_id, kind, value, verified, meta, search_text) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING kind, value, verified, meta, created_at",
        )
        .bind(client_id.0)
        .bind(identity.kind.as_str())
        .bind(&identity.value)
        .bind(identity.verified)
        .bind(&identity.meta)
        .bind(search_text(&identity.value, &identity.meta))
        .fetch_one(&mut *self.conn)
        .await?;
        row.try_into()
    }

    pub async fn add_identity_if_free(
        &mut self,
        client_id: ClientId,
        identity: &NewIdentity,
    ) -> StoreResult<bool> {
        let added: Option<i64> = sqlx::query_scalar(
            "INSERT INTO client_identities (client_id, kind, value, verified, meta, search_text) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (kind, value) DO NOTHING RETURNING id",
        )
        .bind(client_id.0)
        .bind(identity.kind.as_str())
        .bind(&identity.value)
        .bind(identity.verified)
        .bind(&identity.meta)
        .bind(search_text(&identity.value, &identity.meta))
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(added.is_some())
    }

    pub async fn update_identity_meta(
        &mut self,
        kind: IdentityKind,
        value: &str,
        meta: &serde_json::Value,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE client_identities SET meta = $3, search_text = $4 WHERE kind = $1 AND value = $2 \
             AND (meta IS DISTINCT FROM $3 OR search_text IS DISTINCT FROM $4)",
        )
        .bind(kind.as_str())
        .bind(value)
        .bind(meta)
        .bind(search_text(value, meta))
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn identities(&mut self, client_id: ClientId) -> StoreResult<Vec<Identity>> {
        let rows: Vec<IdentityRow> = sqlx::query_as(
            "SELECT kind, value, verified, meta, created_at FROM client_identities WHERE client_id = $1 ORDER BY id",
        )
        .bind(client_id.0)
        .fetch_all(&mut *self.conn)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn set_status(
        &mut self,
        id: ClientId,
        status: ClientStatus,
        reason: Option<&str>,
    ) -> StoreResult<Client> {
        let row: ClientRow = sqlx::query_as(&format!(
            "UPDATE clients AS c SET status = $2, block_reason = $3, \
             blocked_at = CASE WHEN $2 = 'blocked' THEN COALESCE(c.blocked_at, now()) ELSE NULL END \
             WHERE c.id = $1 AND c.deleted_at IS NULL RETURNING {COLUMNS}"
        ))
        .bind(id.0)
        .bind(status.as_str())
        .bind(reason)
        .fetch_optional(&mut *self.conn)
        .await?
        .ok_or(StoreError::NotFound("клиент"))?;
        row.try_into()
    }

    pub async fn set_note(&mut self, id: ClientId, note: Option<&str>) -> StoreResult<()> {
        sqlx::query("UPDATE clients SET note = $2 WHERE id = $1")
            .bind(id.0)
            .bind(note)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn receipt_email(&mut self, id: ClientId) -> StoreResult<Option<String>> {
        Ok(sqlx::query_scalar::<_, Option<String>>(
            "SELECT receipt_email FROM clients WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?
        .flatten())
    }

    pub async fn set_receipt_email(
        &mut self,
        id: ClientId,
        email: Option<&str>,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE clients SET receipt_email = $2 WHERE id = $1")
            .bind(id.0)
            .bind(email)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_locale(&mut self, id: ClientId, locale: Locale) -> StoreResult<()> {
        sqlx::query("UPDATE clients SET locale = $2 WHERE id = $1")
            .bind(id.0)
            .bind(locale.as_str())
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn set_bot_unreachable(
        &mut self,
        id: ClientId,
        unreachable: bool,
    ) -> StoreResult<()> {
        sqlx::query("UPDATE clients SET bot_unreachable = $2 WHERE id = $1")
            .bind(id.0)
            .bind(unreachable)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn list(&mut self, page: PageRequest) -> StoreResult<Page<Client>> {
        let rows: Vec<ClientRow> = sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM clients c WHERE c.deleted_at IS NULL AND ($1::bigint IS NULL OR c.id < $1) \
             ORDER BY c.id DESC LIMIT $2"
        ))
        .bind(page.after)
        .bind(page.fetch_limit())
        .fetch_all(&mut *self.conn)
        .await?;
        let items = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<StoreResult<Vec<Client>>>()?;
        Ok(Page::from_overfetch(items, page, |c| c.id.0))
    }

    pub async fn delete_identities(&mut self, id: ClientId) -> StoreResult<u64> {
        Ok(
            sqlx::query("DELETE FROM client_identities WHERE client_id = $1")
                .bind(id.0)
                .execute(&mut *self.conn)
                .await?
                .rows_affected(),
        )
    }

    pub async fn soft_delete(&mut self, id: ClientId) -> StoreResult<()> {
        sqlx::query("UPDATE clients SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL")
            .bind(id.0)
            .execute(&mut *self.conn)
            .await?;
        Ok(())
    }

    pub async fn totp(&mut self, id: ClientId) -> StoreResult<Option<ClientTotp>> {
        let row: Option<TotpRow> = sqlx::query_as(
            "SELECT totp_nonce, totp_secret, totp_enabled_at FROM client_credentials WHERE client_id = $1",
        )
        .bind(id.0)
        .fetch_optional(&mut *self.conn)
        .await?;
        Ok(row.and_then(|row| match (row.totp_nonce, row.totp_secret) {
            (Some(nonce), Some(ciphertext)) => Some(ClientTotp {
                secret: SealedSecret { nonce, ciphertext },
                enabled_at: row.totp_enabled_at,
            }),
            _ => None,
        }))
    }

    pub async fn enable_totp(
        &mut self,
        id: ClientId,
        secret: &SealedSecret,
        at: DateTime<Utc>,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO client_credentials (client_id, totp_nonce, totp_secret, totp_last_step, totp_enabled_at) \
             VALUES ($1, $2, $3, NULL, $4) \
             ON CONFLICT (client_id) DO UPDATE SET totp_nonce = EXCLUDED.totp_nonce, \
             totp_secret = EXCLUDED.totp_secret, totp_last_step = NULL, totp_enabled_at = EXCLUDED.totp_enabled_at",
        )
        .bind(id.0)
        .bind(&secret.nonce)
        .bind(&secret.ciphertext)
        .bind(at)
        .execute(&mut *self.conn)
        .await?;
        Ok(())
    }

    pub async fn clear_totp(&mut self, id: ClientId) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE client_credentials SET totp_nonce = NULL, totp_secret = NULL, totp_last_step = NULL, \
             totp_enabled_at = NULL WHERE client_id = $1 AND totp_secret IS NOT NULL",
        )
        .bind(id.0)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn use_totp_step(&mut self, id: ClientId, step: i64) -> StoreResult<bool> {
        let done = sqlx::query(
            "UPDATE client_credentials SET totp_last_step = $2 WHERE client_id = $1 \
             AND totp_secret IS NOT NULL AND (totp_last_step IS NULL OR totp_last_step < $2)",
        )
        .bind(id.0)
        .bind(step)
        .execute(&mut *self.conn)
        .await?;
        Ok(done.rows_affected() > 0)
    }
}
