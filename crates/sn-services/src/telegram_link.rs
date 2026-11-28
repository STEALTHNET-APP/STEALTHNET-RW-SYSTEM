use crate::Actor;
use crate::auth::{RequestMeta, token_hash};
use crate::bot_webhook::host_of;
use crate::email_change::masked_email;
use crate::error::{ServiceError, ServiceResult};
use crate::limits::rules;
use crate::login::{TELEGRAM_LOGIN_TTL_SECONDS, forget_old_requests, random_hex};
use crate::oauth::account_title;
use crate::services::Services;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::{Client, Identity, IdentityKind, NewIdentity, TelegramProfile};
use sn_domain::events::DomainEvent;
use sn_domain::ids::ClientId;
use sn_domain::jobs::JobPayload;
use sn_domain::lang;
use sn_domain::settings::keys;
use sn_modules::catalog::start_param::LINK_PREFIX;
use sn_store::{Db, LockSpace, Tx};

pub const START_PREFIX: &str = LINK_PREFIX;
pub const TEMPLATE_LINKED: &str = "telegram_linked";
pub const LINKED_AUDIT: &str = "client.telegram_linked";
pub const MERGED_AUDIT: &str = "client.merged_into";

const LEFT_BEHIND: [&str; 14] = [
    "client_identities",
    "client_sessions",
    "client_login_requests",
    "client_photos",
    "client_utm",
    "referral_codes",
    "notifications_sent",
    "broadcast_recipients",
    "broadcast_unsubscribes",
    "auto_broadcast_cases",
    "antibot_events",
    "oauth_requests",
    "site_signups",
    "app_links",
];
pub const EXPIRED: &str = "telegram_link.expired";
pub const TAKEN: &str = "telegram_link.taken";
pub const ALREADY_LINKED: &str = "telegram_link.already_linked";
pub const UNAVAILABLE: &str = "telegram_link.unavailable";
pub const SESSION_GONE: &str = "telegram_link.session_gone";
pub const BLOCKED: &str = "telegram_link.blocked";
pub const EMAIL_UNVERIFIED: &str = "telegram_link.email_unverified";
pub const EMAIL_UNCONFIRMABLE: &str = "telegram_link.email_unconfirmable";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelegramLinkStart {
    pub poll_token: String,
    pub link: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramLinkPrompt {
    Ask {
        account: String,
        site: String,
        closes_empty: bool,
    },
    Linked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramLinked {
    pub client_id: ClientId,
    pub repeated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TelegramLinkStatus {
    Pending { expires_at: DateTime<Utc> },
    Linked { account: String },
    Rejected,
    Expired,
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: i64,
    status: String,
    client_id: i64,
    session_id: i64,
    ip: Option<String>,
    expires_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, status, client_id, session_id, ip, expires_at";

fn expired() -> ServiceError {
    ServiceError::rejected(
        EXPIRED,
        "Ссылка для привязки устарела. Откройте на сайте «Безопасность» и начните заново.",
    )
}

fn taken() -> ServiceError {
    ServiceError::rejected(
        TAKEN,
        "У этого Telegram уже есть свой аккаунт. Войдите в него или напишите в поддержку.",
    )
}

fn already_linked() -> ServiceError {
    ServiceError::rejected(ALREADY_LINKED, "К аккаунту уже привязан Telegram.")
}

fn unavailable() -> ServiceError {
    ServiceError::rejected(UNAVAILABLE, "Привязка Telegram сейчас недоступна.")
}

fn session_gone() -> ServiceError {
    ServiceError::rejected(
        SESSION_GONE,
        "Сессия кабинета закончилась. Войдите на сайт заново и повторите привязку.",
    )
}

fn blocked() -> ServiceError {
    ServiceError::rejected(
        BLOCKED,
        "Аккаунт заблокирован. Если это ошибка, напишите в поддержку.",
    )
}

fn link_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    let code = code.strip_prefix(START_PREFIX).unwrap_or(code);
    (code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| code.to_ascii_lowercase())
}

fn awaits_confirmation(identity: &Identity) -> bool {
    identity.kind == IdentityKind::Email && !identity.verified && !identity.linked_without_code()
}

fn telegram(identities: &[Identity]) -> Option<&Identity> {
    identities.iter().find(|i| i.kind == IdentityKind::Telegram)
}

fn masked_account(identities: &[Identity]) -> String {
    let email = identities
        .iter()
        .find(|i| i.kind == IdentityKind::Email)
        .map(|i| masked_email(&i.value));
    let provider = || {
        identities
            .iter()
            .find(|i| matches!(i.kind, IdentityKind::Google | IdentityKind::Apple))
            .map(|i| {
                let title = account_title(i);
                if title.contains('@') {
                    masked_email(&title)
                } else {
                    title
                }
            })
    };
    email
        .or_else(provider)
        .unwrap_or_else(|| lang::pick("без почты", "no email").to_string())
}

async fn session_alive(db: &mut impl Db, row: &LinkRow, now: DateTime<Utc>) -> ServiceResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM client_sessions WHERE id = $1 AND client_id = $2 \
         AND channel = 'cabinet' AND scope = 'full' AND revoked_at IS NULL AND expires_at > $3)",
    )
    .bind(row.session_id)
    .bind(row.client_id)
    .bind(now)
    .fetch_one(db.conn())
    .await?)
}

async fn request(db: &mut impl Db, code: &str, lock: &str) -> ServiceResult<Option<LinkRow>> {
    let Some(code) = link_code(code) else {
        return Ok(None);
    };
    Ok(sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM client_login_requests \
         WHERE method = 'telegram_link' AND code_hash = $1 {lock}"
    ))
    .bind(token_hash(&code))
    .fetch_optional(db.conn())
    .await?)
}

async fn is_empty(db: &mut impl Db, client: &Client) -> ServiceResult<bool> {
    if client.is_blocked() {
        return Ok(false);
    }
    let identities = db.clients().identities(client.id).await?;
    if identities.iter().any(|i| i.kind != IdentityKind::Telegram) {
        return Ok(false);
    }
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM domain_events WHERE client_id = $1 AND dispatched_at IS NULL)",
    )
    .bind(client.id.0)
    .fetch_one(db.conn())
    .await?;
    if pending {
        return Ok(false);
    }
    let checks: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.conrelid::regclass::text, \
                format('SELECT EXISTS (SELECT 1 FROM %s WHERE %I = $1)', c.conrelid::regclass, a.attname) \
         FROM pg_constraint c \
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey) \
         WHERE c.contype = 'f' AND c.confrelid = 'clients'::regclass",
    )
    .fetch_all(db.conn())
    .await?;
    for (table, query) in checks {
        if LEFT_BEHIND.contains(&table.as_str()) {
            continue;
        }
        let used: bool = if table == "client_balances" {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM client_balances WHERE client_id = $1 AND amount_minor <> 0)",
            )
            .bind(client.id.0)
            .fetch_one(db.conn())
            .await?
        } else {
            sqlx::query_scalar(&query)
                .bind(client.id.0)
                .fetch_one(db.conn())
                .await?
        };
        if used {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn close_empty(tx: &mut Tx, empty: ClientId, into: ClientId) -> ServiceResult<()> {
    let identities = tx.clients().delete_identities(empty).await?;
    sqlx::query("DELETE FROM client_balances WHERE client_id = $1 AND amount_minor = 0")
        .bind(empty.0)
        .execute(tx.conn())
        .await?;
    sqlx::query("DELETE FROM analytics_clients WHERE client_id = $1")
        .bind(empty.0)
        .execute(tx.conn())
        .await?;
    sqlx::query("DELETE FROM clients WHERE id = $1")
        .bind(empty.0)
        .execute(tx.conn())
        .await?;
    tx.audit()
        .record(&Actor::Client(into).audit(
            MERGED_AUDIT,
            "client",
            Some(empty.0),
            serde_json::json!({ "into": into.0, "identities": identities }),
        ))
        .await?;
    Ok(())
}

async fn notify_linked(
    tx: &mut Tx,
    client_id: ClientId,
    profile: &TelegramProfile,
) -> ServiceResult<()> {
    let account = match &profile.username {
        Some(username) => format!("@{username}"),
        None => format!("Telegram {}", profile.id),
    };
    tx.jobs()
        .enqueue(
            &JobPayload::Notify {
                client_id,
                template: TEMPLATE_LINKED.into(),
                params: serde_json::json!({ "provider": "Telegram", "account": account }),
            },
            None,
        )
        .await?;
    Ok(())
}

pub struct TelegramLinks<'a> {
    services: &'a Services,
}

impl<'a> TelegramLinks<'a> {
    pub fn new(services: &'a Services) -> Self {
        TelegramLinks { services }
    }

    async fn bot(&self, db: &mut impl Db) -> ServiceResult<Option<String>> {
        if !self.services.settings().client_access.site_login || self.services.bot_token().is_none()
        {
            return Ok(None);
        }
        Ok(db
            .settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .map(|name| name.trim().trim_start_matches('@').to_string())
            .filter(|name| !name.is_empty()))
    }

    pub(crate) async fn offered(
        &self,
        db: &mut impl Db,
        identities: &[Identity],
    ) -> ServiceResult<bool> {
        Ok(telegram(identities).is_none() && self.bot(db).await?.is_some())
    }

    fn check_email(&self, identities: &[Identity]) -> ServiceResult<()> {
        if !identities.iter().any(awaits_confirmation) {
            return Ok(());
        }
        Err(if self.services.mail().available() {
            ServiceError::rejected(
                EMAIL_UNVERIFIED,
                "Сначала подтвердите почту: войдите по коду из письма или восстановите пароль через «Забыли пароль». После этого привяжите Telegram.",
            )
        } else {
            ServiceError::rejected(
                EMAIL_UNCONFIRMABLE,
                "Сначала нужно подтвердить почту, а письма сейчас не отправляются. Напишите в поддержку.",
            )
        })
    }

    pub async fn start(
        &self,
        principal: &ClientPrincipal,
        meta: &RequestMeta,
    ) -> ServiceResult<TelegramLinkStart> {
        if principal.channel != ClientChannel::Cabinet || !principal.full_access {
            return Err(ServiceError::Forbidden);
        }
        let session = principal.session_id.ok_or(ServiceError::Forbidden)?;
        let bot = {
            let mut conn = self.services.store().conn().await?;
            let identities = conn.clients().identities(principal.client_id).await?;
            let bot = self.bot(&mut conn).await?.ok_or_else(unavailable)?;
            if telegram(&identities).is_some() {
                return Err(already_linked());
            }
            self.check_email(&identities)?;
            bot
        };
        let limits = self.services.limits();
        limits
            .hit(
                rules::TELEGRAM_LINK_CLIENT,
                &principal.client_id.to_string(),
            )
            .await?;
        limits
            .hit_ip(rules::TELEGRAM_LINK_IP, meta.ip.as_deref())
            .await?;
        let code = random_hex(16);
        let poll_token = random_hex(32);
        let now = self.services.now();
        let expires_at = now + Duration::seconds(TELEGRAM_LOGIN_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        tx.clients().lock(principal.client_id).await?;
        sqlx::query(
            "UPDATE client_login_requests SET expires_at = $2 \
             WHERE method = 'telegram_link' AND client_id = $1 AND status = 'pending' AND expires_at > $2",
        )
        .bind(principal.client_id.0)
        .bind(now)
        .execute(tx.conn())
        .await?;
        sqlx::query(
            "INSERT INTO client_login_requests \
             (method, code_hash, poll_hash, client_id, session_id, ip, user_agent, created_at, expires_at) \
             VALUES ('telegram_link', $1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(token_hash(&code))
        .bind(token_hash(&poll_token))
        .bind(principal.client_id.0)
        .bind(session)
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        let mut conn = self.services.store().conn().await?;
        forget_old_requests(&mut conn, now).await?;
        Ok(TelegramLinkStart {
            poll_token,
            link: format!("https://t.me/{bot}?start={START_PREFIX}{code}"),
            expires_at,
        })
    }

    async fn owned(
        &self,
        db: &mut impl Db,
        principal: &ClientPrincipal,
        poll_token: &str,
    ) -> ServiceResult<LinkRow> {
        let session = principal.session_id.ok_or(ServiceError::Forbidden)?;
        if poll_token.trim().is_empty() {
            return Err(ServiceError::NotFound("запрос привязки"));
        }
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM client_login_requests \
             WHERE method = 'telegram_link' AND poll_hash = $1 AND client_id = $2 AND session_id = $3"
        ))
        .bind(token_hash(poll_token))
        .bind(principal.client_id.0)
        .bind(session)
        .fetch_optional(db.conn())
        .await?
        .ok_or(ServiceError::NotFound("запрос привязки"))
    }

    pub async fn status(
        &self,
        principal: &ClientPrincipal,
        poll_token: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<TelegramLinkStatus> {
        self.services
            .limits()
            .hit_ip(rules::TELEGRAM_POLL_IP, meta.ip.as_deref())
            .await?;
        let mut conn = self.services.store().conn().await?;
        let row = self.owned(&mut conn, principal, poll_token).await?;
        Ok(match row.status.as_str() {
            "confirmed" => {
                let identities = conn.clients().identities(principal.client_id).await?;
                match telegram(&identities) {
                    Some(identity) => TelegramLinkStatus::Linked {
                        account: account_title(identity),
                    },
                    None => TelegramLinkStatus::Expired,
                }
            }
            "rejected" => TelegramLinkStatus::Rejected,
            "pending" if row.expires_at > self.services.now() => TelegramLinkStatus::Pending {
                expires_at: row.expires_at,
            },
            _ => TelegramLinkStatus::Expired,
        })
    }

    pub async fn stop(&self, principal: &ClientPrincipal, poll_token: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        let row = self.owned(&mut conn, principal, poll_token).await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' WHERE id = $1 AND status = 'pending'",
        )
        .bind(row.id)
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    async fn attempt(&self, profile: &TelegramProfile) -> ServiceResult<()> {
        self.services
            .limits()
            .hit(rules::TELEGRAM_LINK_USER, &profile.id.to_string())
            .await?;
        Ok(())
    }

    async fn owner(
        &self,
        db: &mut impl Db,
        profile: &TelegramProfile,
    ) -> ServiceResult<Option<Client>> {
        Ok(db
            .clients()
            .find_by_identity(IdentityKind::Telegram, &profile.id.to_string())
            .await?)
    }

    pub async fn prompt(
        &self,
        code: &str,
        profile: &TelegramProfile,
    ) -> ServiceResult<TelegramLinkPrompt> {
        self.attempt(profile).await?;
        let now = self.services.now();
        let mut conn = self.services.store().conn().await?;
        let row = request(&mut conn, code, "").await?.ok_or_else(expired)?;
        let client_id = ClientId(row.client_id);
        let owner = self.owner(&mut conn, profile).await?;
        let mine = owner.as_ref().is_some_and(|o| o.id == client_id);
        if row.status == "confirmed" && mine {
            return Ok(TelegramLinkPrompt::Linked);
        }
        if row.status != "pending" || row.expires_at <= now {
            return Err(expired());
        }
        if mine {
            return Ok(TelegramLinkPrompt::Linked);
        }
        let closes_empty = match &owner {
            Some(owner) if is_empty(&mut conn, owner).await? => true,
            Some(_) => return Err(taken()),
            None => false,
        };
        if self.bot(&mut conn).await?.is_none() {
            return Err(unavailable());
        }
        if !session_alive(&mut conn, &row, now).await? {
            return Err(session_gone());
        }
        if conn.clients().get(client_id).await?.is_blocked() {
            return Err(blocked());
        }
        let identities = conn.clients().identities(client_id).await?;
        if telegram(&identities).is_some() {
            return Err(already_linked());
        }
        self.check_email(&identities)?;
        let settings = self.services.settings();
        let site = settings
            .public_url
            .as_deref()
            .and_then(host_of)
            .unwrap_or_else(|| settings.brand_name.clone());
        Ok(TelegramLinkPrompt::Ask {
            account: masked_account(&identities),
            site,
            closes_empty,
        })
    }

    pub async fn confirm(
        &self,
        code: &str,
        profile: &TelegramProfile,
    ) -> ServiceResult<TelegramLinked> {
        self.attempt(profile).await?;
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let client_id = request(&mut tx, code, "")
            .await?
            .map(|row| ClientId(row.client_id))
            .ok_or_else(expired)?;
        tx.lock(LockSpace::Telegram, profile.id).await?;
        let found = self.owner(&mut tx, profile).await?;
        let mut owner = None;
        let mut client = None;
        let mut ids = vec![client_id];
        ids.extend(found.as_ref().map(|o| o.id).filter(|id| *id != client_id));
        ids.sort_by_key(|id| id.0);
        for id in ids {
            let locked = tx.clients().lock(id).await?;
            if id == client_id {
                client = Some(locked);
            } else {
                owner = Some(locked);
            }
        }
        let client = client.ok_or_else(expired)?;
        if found.as_ref().is_some_and(|o| o.id == client_id) {
            owner = Some(client.clone());
        }
        let row = request(&mut tx, code, "FOR UPDATE")
            .await?
            .ok_or_else(expired)?;
        let mine = owner.as_ref().is_some_and(|o| o.id == client_id);
        let repeated = TelegramLinked {
            client_id,
            repeated: true,
        };
        if row.status == "confirmed" && mine {
            return Ok(repeated);
        }
        if row.status != "pending" || row.expires_at <= now {
            return Err(expired());
        }
        let empty = match &owner {
            Some(other) if !mine => {
                if !is_empty(&mut tx, other).await? {
                    return Err(taken());
                }
                Some(other.id)
            }
            _ => None,
        };
        if self.bot(&mut tx).await?.is_none() {
            return Err(unavailable());
        }
        if !session_alive(&mut tx, &row, now).await? {
            return Err(session_gone());
        }
        if client.is_blocked() {
            return Err(blocked());
        }
        if !mine {
            let identities = tx.clients().identities(client_id).await?;
            if telegram(&identities).is_some() {
                return Err(already_linked());
            }
            self.check_email(&identities)?;
            if let Some(empty) = empty {
                close_empty(&mut tx, empty, client_id).await?;
            }
            if !tx
                .clients()
                .add_identity_if_free(client_id, &NewIdentity::telegram(profile))
                .await?
            {
                return Err(taken());
            }
            tx.clients().set_bot_unreachable(client_id, false).await?;
            notify_linked(&mut tx, client_id, profile).await?;
            let mut entry = Actor::Client(client_id).audit(
                LINKED_AUDIT,
                "client",
                Some(client_id.0),
                serde_json::json!({
                    "telegram_id": profile.id,
                    "username": profile.username,
                    "closed_client": empty.map(|id| id.0),
                }),
            );
            entry.ip = row.ip.clone();
            tx.audit().record(&entry).await?;
            tx.events()
                .push(&DomainEvent::ClientTelegramLinked {
                    client_id,
                    telegram_id: profile.id,
                })
                .await?;
        }
        sqlx::query(
            "UPDATE client_login_requests SET status = 'confirmed', confirmed_at = $2 WHERE id = $1",
        )
        .bind(row.id)
        .bind(now)
        .execute(tx.conn())
        .await?;
        tx.commit().await?;
        Ok(TelegramLinked {
            client_id,
            repeated: mine,
        })
    }

    pub async fn reject(&self, code: &str, profile: &TelegramProfile) -> ServiceResult<()> {
        self.attempt(profile).await?;
        let code = link_code(code).ok_or_else(expired)?;
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = 'telegram_link' AND code_hash = $1 AND status = 'pending'",
        )
        .bind(token_hash(&code))
        .execute(conn.conn())
        .await?;
        Ok(())
    }
}
