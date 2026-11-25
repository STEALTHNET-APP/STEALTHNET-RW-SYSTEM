use crate::Actor;
use crate::auth::{
    Auth, CLIENT_SESSION_DAYS, ClientLogin, RequestMeta, check_password, hash_password_async,
    token_hash,
};
use crate::clients::{Entered, SiteVisit};
use crate::error::{ServiceError, ServiceResult};
use crate::limits::{RateLimits, Strike, rules};
use crate::mail::SignupRule;
use crate::signup::{Arrival, confirm_required, signup_closed};
use crate::site_signup;
use crate::two_factor::{CabinetLogin, cabinet_session, cabinet_session_for};
use crate::utm_tracking::site_start_param;
use chrono::{DateTime, Duration, Utc};
use rand::{Rng, RngCore};
use serde::Serialize;
use sn_domain::access::{ClientChannel, ClientPrincipal};
use sn_domain::client::{
    IdentityKind, NewIdentity, RegistrationSource, TelegramProfile, normalize_identity,
};
use sn_domain::events::DomainEvent;
use sn_domain::format;
use sn_domain::ids::ClientId;
use sn_domain::jobs::JobPayload;
use sn_domain::lang;
use sn_domain::mail_letters::{LetterId, Values};
use sn_domain::settings::keys;
use sn_modules::catalog::antibot;
use sn_modules::catalog::antibot::signups::{self, Refusal, Via};
use sn_modules::catalog::start_param::Utm;
use sn_store::{Db, NewClient, NewSession, SessionChannel, SessionScope, Tx};

pub const TELEGRAM_LOGIN_TTL_SECONDS: i64 = 300;
pub const TELEGRAM_START_PREFIX: &str = "auth_";
pub const EMAIL_CODE_TTL_SECONDS: i64 = 900;

pub(crate) fn code_values(code: String) -> Values {
    let minutes = u64::try_from(EMAIL_CODE_TTL_SECONDS / 60).unwrap_or(15);
    Values::new().with("code", code).with(
        "minutes",
        format::count_in(
            minutes,
            lang::current(),
            ["минута", "минуты", "минут"],
            ["minute", "minutes"],
        ),
    )
}
pub const EMAIL_CODE_MAX_ATTEMPTS: i32 = 5;
pub const EMAIL_CODE_DIGITS: usize = 6;
pub const LOGIN_RETENTION_DAYS: i64 = 7;
const LOGIN_SWEEP: i64 = 500;
const METHOD_EMAIL: &str = "email";
pub const LOGIN_LETTERS: &str = "code_letters_login";
pub const RESET_LETTERS: &str = "code_letters_reset";
pub const CHANGE_LETTERS: &str = "code_letters_change";
const METHOD_RESET: &str = "reset";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelegramLoginStart {
    pub poll_token: String,
    pub link: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelegramLoginPrompt {
    pub account: String,
    pub ip: Option<String>,
    pub device: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl TelegramLoginPrompt {
    pub fn text(&self) -> String {
        let place = match (&self.ip, &self.device) {
            (Some(ip), Some(device)) => format!("{ip}, {device}"),
            (Some(ip), None) => ip.clone(),
            (None, Some(device)) => device.clone(),
            (None, None) => "неизвестно".to_string(),
        };
        format!(
            "Вход в личный кабинет\nАккаунт: {}\nОткуда: {}\nЗапрос создан в {} UTC.\n\nЕсли входите не вы, нажмите «Это не я».",
            self.account,
            place,
            self.requested_at.format("%H:%M"),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TelegramLoginConfirmed {
    pub client_id: ClientId,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TelegramLoginStatus {
    Pending {
        expires_at: DateTime<Utc>,
    },
    Confirmed {
        login: ClientLogin,
    },
    TotpRequired {
        challenge: String,
        expires_at: DateTime<Utc>,
    },
    Rejected,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailLoginStart {
    pub email: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: i64,
    status: String,
    client_id: Option<i64>,
    code_hash: Vec<u8>,
    attempts: i32,
    ip: Option<String>,
    user_agent: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

const LOGIN_COLUMNS: &str =
    "id, status, client_id, code_hash, attempts, ip, user_agent, created_at, expires_at";

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    hex::encode(raw)
}

pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn link_code(raw: &str) -> Option<String> {
    let code = raw.trim();
    let code = code.strip_prefix(TELEGRAM_START_PREFIX).unwrap_or(code);
    (code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| code.to_ascii_lowercase())
}

pub fn referral_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    ((4..=16).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(code)
}

fn expired_link() -> ServiceError {
    ServiceError::rejected(
        "auth.login_expired",
        "Ссылка для входа устарела. Начните вход на сайте заново.",
    )
}

pub(crate) fn email_code() -> String {
    let mut rng = rand::rngs::OsRng;
    (0..EMAIL_CODE_DIGITS)
        .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
        .collect()
}

pub(crate) fn code_invalid() -> ServiceError {
    ServiceError::rejected("auth.code_invalid", "Код не подошёл или устарел")
}

pub fn describe_user_agent(user_agent: &str) -> Option<String> {
    let browser = if user_agent.contains("YaBrowser") {
        Some(lang::pick("Яндекс Браузер", "Yandex Browser"))
    } else if user_agent.contains("Edg/") {
        Some("Edge")
    } else if user_agent.contains("OPR/") || user_agent.contains("Opera") {
        Some("Opera")
    } else if user_agent.contains("Firefox/") || user_agent.contains("FxiOS") {
        Some("Firefox")
    } else if user_agent.contains("Chrome/") || user_agent.contains("CriOS") {
        Some("Chrome")
    } else if user_agent.contains("Safari/") {
        Some("Safari")
    } else {
        None
    };
    let os = if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        Some("iOS")
    } else if user_agent.contains("Android") {
        Some("Android")
    } else if user_agent.contains("Windows") {
        Some("Windows")
    } else if user_agent.contains("Mac OS X") || user_agent.contains("Macintosh") {
        Some("macOS")
    } else if user_agent.contains("Linux") {
        Some("Linux")
    } else {
        None
    };
    match (browser, os) {
        (Some(browser), Some(os)) => Some(format!("{browser}, {os}")),
        (Some(single), None) | (None, Some(single)) => Some(single.to_string()),
        (None, None) => None,
    }
}

fn account_title(profile: &TelegramProfile) -> String {
    let name = profile.display_name();
    match &profile.username {
        Some(username) if !name.starts_with('@') => format!("{name} (@{username})"),
        _ => name,
    }
}

pub(crate) async fn issue_session_scoped(
    tx: &mut Tx,
    client_id: ClientId,
    now: DateTime<Utc>,
    meta: &RequestMeta,
    created: bool,
    scope: SessionScope,
) -> ServiceResult<ClientLogin> {
    let (token, hash) = crate::auth::new_token();
    let expires_at = now + Duration::days(CLIENT_SESSION_DAYS);
    let session_id = tx
        .sessions()
        .create_client(
            client_id,
            SessionChannel::Cabinet,
            scope,
            &NewSession {
                token_hash: hash,
                expires_at,
                ip: meta.ip.clone(),
                user_agent: meta.user_agent.clone(),
            },
        )
        .await?;
    Ok(ClientLogin {
        token,
        expires_at,
        principal: ClientPrincipal {
            client_id,
            channel: ClientChannel::Cabinet,
            full_access: true,
            session_id: Some(session_id),
        },
        created,
    })
}

pub(crate) async fn forget_old_requests(db: &mut impl Db, now: DateTime<Utc>) -> ServiceResult<()> {
    sqlx::query(
        "DELETE FROM client_login_requests WHERE id IN ( \
             SELECT id FROM client_login_requests WHERE expires_at < $1 \
             ORDER BY expires_at LIMIT $2 FOR UPDATE SKIP LOCKED)",
    )
    .bind(now - Duration::days(LOGIN_RETENTION_DAYS))
    .bind(LOGIN_SWEEP)
    .execute(db.conn())
    .await?;
    Ok(())
}

impl<'a> Auth<'a> {
    async fn bot_username(&self) -> ServiceResult<String> {
        let mut conn = self.services.store().conn().await?;
        conn.settings()
            .get_as::<String>(keys::BOT_USERNAME)
            .await?
            .map(|name| name.trim().trim_start_matches('@').to_string())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                ServiceError::rejected(
                    "bot.not_configured",
                    "Вход через Telegram недоступен: в настройках не указан бот",
                )
            })
    }

    pub async fn telegram_login_start(
        &self,
        meta: &RequestMeta,
    ) -> ServiceResult<TelegramLoginStart> {
        self.telegram_login_start_with(meta, None).await
    }

    pub(crate) fn site_open(&self) -> ServiceResult<()> {
        if self.services.settings().client_access.site_login {
            Ok(())
        } else {
            Err(ServiceError::rejected(
                "auth.site_off",
                "Вход на сайт выключен: пользуйтесь ботом",
            ))
        }
    }

    pub async fn telegram_login_start_with(
        &self,
        meta: &RequestMeta,
        referral: Option<&str>,
    ) -> ServiceResult<TelegramLoginStart> {
        self.site_open()?;
        let limits = self.services.limits();
        limits
            .hit_ip(limits.telegram_start(), meta.ip.as_deref())
            .await?;
        let bot = self.bot_username().await?;
        let code = random_hex(16);
        let poll_token = random_hex(32);
        let now = self.services.now();
        let expires_at = now + Duration::seconds(TELEGRAM_LOGIN_TTL_SECONDS);
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "INSERT INTO client_login_requests (method, code_hash, poll_hash, ip, user_agent, created_at, expires_at) \
             VALUES ('telegram', $1, $2, $3, $4, $5, $6)",
        )
        .bind(token_hash(&code))
        .bind(token_hash(&poll_token))
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .execute(conn.conn())
        .await?;
        forget_old_requests(&mut conn, now).await?;
        Ok(TelegramLoginStart {
            poll_token,
            link: format!(
                "https://t.me/{bot}?start={TELEGRAM_START_PREFIX}{code}{}",
                referral
                    .and_then(referral_code)
                    .map(|c| format!("__ref_{c}"))
                    .unwrap_or_default()
            ),
            expires_at,
        })
    }

    async fn pending_telegram_request(&self, code: &str) -> ServiceResult<LoginRow> {
        let code = link_code(code).ok_or(ServiceError::NotFound("запрос входа"))?;
        let mut conn = self.services.store().conn().await?;
        let row: LoginRow = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests WHERE method = 'telegram' AND code_hash = $1"
        ))
        .bind(token_hash(&code))
        .fetch_optional(conn.conn())
        .await?
        .ok_or(ServiceError::NotFound("запрос входа"))?;
        if row.status != "pending" || row.expires_at <= self.services.now() {
            return Err(expired_link());
        }
        Ok(row)
    }

    pub async fn telegram_login_visit(&self, code: &str) -> ServiceResult<Option<SiteVisit>> {
        match self.pending_telegram_request(code).await {
            Ok(row) => Ok(Some(SiteVisit { ip: row.ip })),
            Err(ServiceError::NotFound(_) | ServiceError::Rejected { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub async fn telegram_login_prompt(
        &self,
        code: &str,
        profile: &TelegramProfile,
    ) -> ServiceResult<TelegramLoginPrompt> {
        let row = self.pending_telegram_request(code).await?;
        Ok(TelegramLoginPrompt {
            account: account_title(profile),
            ip: row.ip,
            device: row.user_agent.as_deref().and_then(describe_user_agent),
            requested_at: row.created_at,
            expires_at: row.expires_at,
        })
    }

    pub async fn confirm_telegram_login(
        &self,
        code: &str,
        profile: &TelegramProfile,
    ) -> ServiceResult<TelegramLoginConfirmed> {
        let pending = self.pending_telegram_request(code).await?;
        let visit = SiteVisit {
            ip: pending.ip.clone(),
        };
        let (client, created) = match self
            .services
            .clients()
            .ensure_telegram_from(profile, RegistrationSource::Web, None, Some(&visit), || {
                true
            })
            .await?
        {
            Entered::Ready(client, created) => (client, created),
            Entered::Busy => {
                return Err(ServiceError::Internal("регистрация не допущена".into()));
            }
            Entered::Refused(refusal) => return Err(site_signup::refusal(refusal)),
        };
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: LoginRow = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests WHERE id = $1 FOR UPDATE"
        ))
        .bind(pending.id)
        .fetch_one(tx.conn())
        .await?;
        if row.status != "pending" || row.expires_at <= now {
            return Err(expired_link());
        }
        sqlx::query(
            "UPDATE client_login_requests SET status = 'confirmed', client_id = $2, confirmed_at = $3 WHERE id = $1",
        )
        .bind(row.id)
        .bind(client.id.0)
        .bind(now)
        .execute(tx.conn())
        .await?;
        tx.audit()
            .record(&Actor::Client(client.id).audit(
                "client.telegram_login_confirmed",
                "client",
                Some(client.id.0),
                serde_json::json!({ "ip": row.ip }),
            ))
            .await?;
        tx.commit().await?;
        Ok(TelegramLoginConfirmed {
            client_id: client.id,
            created,
        })
    }

    pub async fn reject_telegram_login(&self, code: &str) -> ServiceResult<()> {
        let code = link_code(code).ok_or(ServiceError::NotFound("запрос входа"))?;
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = 'telegram' AND code_hash = $1 AND status = 'pending'",
        )
        .bind(token_hash(&code))
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    pub async fn telegram_login_status(
        &self,
        poll_token: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<TelegramLoginStatus> {
        self.services
            .limits()
            .hit_ip(rules::TELEGRAM_POLL_IP, meta.ip.as_deref())
            .await?;
        if poll_token.trim().is_empty() {
            return Err(ServiceError::NotFound("запрос входа"));
        }
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: LoginRow = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests \
             WHERE method = 'telegram' AND poll_hash = $1 FOR UPDATE"
        ))
        .bind(token_hash(poll_token))
        .fetch_optional(tx.conn())
        .await?
        .ok_or(ServiceError::NotFound("запрос входа"))?;
        let grace = Duration::seconds(TELEGRAM_LOGIN_TTL_SECONDS);
        let status = match row.status.as_str() {
            "pending" if row.expires_at > now => TelegramLoginStatus::Pending {
                expires_at: row.expires_at,
            },
            "rejected" => TelegramLoginStatus::Rejected,
            "confirmed" if row.expires_at + grace > now => {
                let client_id = ClientId(row.client_id.ok_or(ServiceError::Internal(
                    "подтверждённый вход без клиента".into(),
                ))?);
                let client = tx.clients().get(client_id).await?;
                if client.is_blocked() {
                    return Err(ServiceError::Forbidden);
                }
                sqlx::query(
                    "UPDATE client_login_requests SET status = 'consumed', consumed_at = $2 WHERE id = $1",
                )
                .bind(row.id)
                .bind(now)
                .execute(tx.conn())
                .await?;
                let status = match cabinet_session(&mut tx, client_id, now, meta, false).await? {
                    CabinetLogin::Session(login) => TelegramLoginStatus::Confirmed { login },
                    CabinetLogin::TotpRequired(totp) => TelegramLoginStatus::TotpRequired {
                        challenge: totp.challenge,
                        expires_at: totp.expires_at,
                    },
                };
                tx.commit().await?;
                return Ok(status);
            }
            _ => TelegramLoginStatus::Expired,
        };
        tx.commit().await?;
        Ok(status)
    }

    pub async fn email_login_start(
        &self,
        email: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailLoginStart> {
        self.email_login_start_with(email, meta, None).await
    }

    pub async fn email_login_start_with(
        &self,
        email: &str,
        meta: &RequestMeta,
        captcha: Option<&str>,
    ) -> ServiceResult<EmailLoginStart> {
        self.site_open()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(limits.email_start(), meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        limits
            .hit_wide(limits.email_start_wide(), meta.ip.as_deref())
            .await?;
        let mail = self.services.mail();
        if !mail.code_login() {
            return Err(ServiceError::rejected(
                "auth.email_password_mode",
                "Вход по почте — по паролю, без кода",
            ));
        }
        self.captcha_gate(antibot::LOGIN, captcha, meta).await?;
        limits
            .letters_sent(LOGIN_LETTERS, mail.codes_per_hour())
            .await?;
        let (code, start) = self.issue_email_code(&email, meta).await?;
        self.services
            .letters()
            .send(LetterId::LoginCode, &start.email, code_values(code))
            .await?;
        Ok(start)
    }

    pub(crate) fn email_by_password(&self) -> ServiceResult<()> {
        if self.services.mail().code_login() {
            Err(ServiceError::rejected(
                "auth.email_code_mode",
                "Вход по почте — кодом из письма",
            ))
        } else {
            Ok(())
        }
    }

    pub async fn email_register(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.email_register_with(email, password, meta, None).await
    }

    pub async fn email_register_with(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
        utm: Option<&Utm>,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        self.email_by_password()?;
        match self.services.mail().signup_rule() {
            SignupRule::Open => {}
            SignupRule::Confirm => return Err(confirm_required()),
            SignupRule::Closed => return Err(signup_closed()),
        }
        self.captcha_gate(antibot::SIGNUP, None, meta).await?;
        self.register_now(email, password, meta, site_start_param(utm, None), None)
            .await
    }

    pub(crate) async fn captcha_gate(
        &self,
        action: &str,
        token: Option<&str>,
        meta: &RequestMeta,
    ) -> ServiceResult<()> {
        let ctx = self.services.module_context();
        if antibot::signup_captcha(&ctx).is_none() {
            return Ok(());
        }
        let limits = self.services.limits();
        limits.hit_ip(limits.captcha(), meta.ip.as_deref()).await?;
        match antibot::check_signup(&ctx, action, token, meta.ip.as_deref()).await? {
            antibot::SignupCheck::Off | antibot::SignupCheck::Passed => Ok(()),
            check => Err(crate::site_signup::captcha_error(check)),
        }
    }

    pub(crate) async fn register_now(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
        start_param: Option<String>,
        captcha: Option<&str>,
    ) -> ServiceResult<CabinetLogin> {
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(limits.email_start(), meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        limits
            .hit_wide(limits.email_start_wide(), meta.ip.as_deref())
            .await?;
        let mut human = site_signup::asked_before(self.services);
        let early = {
            let mut conn = self.services.store().conn().await?;
            if conn
                .clients()
                .find_by_identity(IdentityKind::Email, &email)
                .await?
                .is_some()
            {
                return Err(site_signup::email_taken());
            }
            signups::refuse_early(&mut conn, meta.ip.as_deref(), human).await?
        };
        if let Some(refusal) = early {
            site_signup::over_limit(self.services, refusal, captcha, meta).await?;
            human = true;
        }
        let hash = hash_password_async(password).await?;
        loop {
            match self
                .create_by_password(&email, &hash, meta, start_param.clone(), human)
                .await?
            {
                Ok(login) => return Ok(login),
                Err(refusal) => {
                    site_signup::over_limit(self.services, refusal, captcha, meta).await?;
                    human = true;
                }
            }
        }
    }

    async fn create_by_password(
        &self,
        email: &str,
        hash: &str,
        meta: &RequestMeta,
        start_param: Option<String>,
        human: bool,
    ) -> ServiceResult<Result<CabinetLogin, Refusal>> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        if tx
            .clients()
            .find_by_identity(IdentityKind::Email, email)
            .await?
            .is_some()
        {
            return Err(site_signup::email_taken());
        }
        let admission = match signups::admit(&mut tx, meta.ip.as_deref(), human).await? {
            Ok(admission) => admission,
            Err(refusal) => {
                tx.rollback().await?;
                return Ok(Err(refusal));
            }
        };
        let client = tx
            .clients()
            .create(&NewClient {
                locale: self.services.settings().languages.pick(lang::hint()),
                registration_source: RegistrationSource::Web,
                note: None,
            })
            .await?;
        tx.clients()
            .add_identity(
                client.id,
                &NewIdentity::new(IdentityKind::Email, email, false)?,
            )
            .await?;
        sqlx::query("INSERT INTO client_passwords (client_id, password_hash) VALUES ($1, $2)")
            .bind(client.id.0)
            .bind(hash)
            .execute(tx.conn())
            .await?;
        signups::record(&mut tx, admission, client.id, Via::Password).await?;
        tx.events()
            .push(&DomainEvent::ClientRegistered {
                client_id: client.id,
                source: RegistrationSource::Web,
                start_param,
            })
            .await?;
        let login = cabinet_session(&mut tx, client.id, now, meta, true).await?;
        tx.commit().await?;
        Ok(Ok(login))
    }

    pub async fn email_password_login(
        &self,
        email: &str,
        password: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        self.email_by_password()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_VERIFY_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let (client, hash) = {
            let mut conn = self.services.store().conn().await?;
            let client = conn
                .clients()
                .find_by_identity(IdentityKind::Email, &email)
                .await?;
            let stored: Option<(String, bool)> = match &client {
                Some(client) => sqlx::query_as(
                    "SELECT password_hash, must_change FROM client_passwords WHERE client_id = $1",
                )
                .bind(client.id.0)
                .fetch_optional(conn.conn())
                .await?,
                None => None,
            };
            (client, stored)
        };
        let must_change = hash.as_ref().is_some_and(|(_, must_change)| *must_change);
        let legacy = hash
            .as_ref()
            .is_some_and(|(hash, _)| crate::auth::legacy_hash(hash));
        let valid = check_password(password, hash.map(|(hash, _)| hash)).await?;
        let Some(client) = client.filter(|_| valid) else {
            return Err(ServiceError::rejected(
                "auth.wrong_password",
                "Почта или пароль не подошли",
            ));
        };
        if client.is_blocked() {
            return Err(ServiceError::Forbidden);
        }
        let fresh = if legacy {
            hash_password_async(password).await.ok()
        } else {
            None
        };
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        if let Some(fresh) = fresh {
            store_password(&mut tx, client.id, &fresh, must_change).await?;
        }
        let login = cabinet_session_for(&mut tx, client.id, now, meta, false, must_change).await?;
        tx.commit().await?;
        Ok(login)
    }

    pub async fn issue_email_code(
        &self,
        email: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<(String, EmailLoginStart)> {
        self.issue_code(METHOD_EMAIL, email, meta).await
    }

    async fn issue_code(
        &self,
        method: &str,
        email: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<(String, EmailLoginStart)> {
        let email = normalize_identity(IdentityKind::Email, email)?;
        let code = email_code();
        let now = self.services.now();
        let expires_at = now + Duration::seconds(EMAIL_CODE_TTL_SECONDS);
        let mut tx = self.services.store().begin().await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'rejected' \
             WHERE method = $2 AND email = $1 AND status = 'pending'",
        )
        .bind(&email)
        .bind(method)
        .execute(tx.conn())
        .await?;
        sqlx::query(
            "INSERT INTO client_login_requests (method, code_hash, email, ip, user_agent, created_at, expires_at) \
             VALUES ($7, $1, $2, $3, $4, $5, $6)",
        )
        .bind(token_hash(&format!("{email}:{code}")))
        .bind(&email)
        .bind(&meta.ip)
        .bind(&meta.user_agent)
        .bind(now)
        .bind(expires_at)
        .bind(method)
        .execute(tx.conn())
        .await?;
        forget_old_requests(&mut tx, now).await?;
        tx.commit().await?;
        Ok((code, EmailLoginStart { email, expires_at }))
    }

    pub async fn email_login_verify(
        &self,
        email: &str,
        code: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.email_login_verify_with(email, code, meta, None).await
    }

    pub async fn email_login_verify_with(
        &self,
        email: &str,
        code: &str,
        meta: &RequestMeta,
        utm: Option<&Utm>,
    ) -> ServiceResult<CabinetLogin> {
        self.email_login_verify_from(email, code, meta, Arrival::site(utm, None), None)
            .await
    }

    pub async fn email_login_verify_from(
        &self,
        email: &str,
        code: &str,
        meta: &RequestMeta,
        from: Arrival<'_>,
        captcha: Option<&str>,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_VERIFY_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.len() != EMAIL_CODE_DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(code_invalid());
        }
        let mut strike = Some(code_strike(&limits, &email).await?);
        let mut human = site_signup::asked_before(self.services);
        loop {
            match self
                .enter_by_code(&email, &code, meta, &from, strike.as_ref(), human)
                .await?
            {
                Ok(login) => return Ok(login),
                Err(refusal) => {
                    if let Some(strike) = strike.take() {
                        limits.forgive(&strike).await?;
                    }
                    site_signup::over_limit(self.services, refusal, captcha, meta).await?;
                    human = true;
                }
            }
        }
    }

    async fn enter_by_code(
        &self,
        email: &str,
        code: &str,
        meta: &RequestMeta,
        from: &Arrival<'_>,
        strike: Option<&Strike>,
        human: bool,
    ) -> ServiceResult<Result<CabinetLogin, Refusal>> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<LoginRow> = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests \
             WHERE method = 'email' AND email = $1 AND status = 'pending' ORDER BY id DESC LIMIT 1 FOR UPDATE"
        ))
        .bind(email)
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row.filter(|r| r.expires_at > now) else {
            return Err(code_invalid());
        };
        if row.attempts >= EMAIL_CODE_MAX_ATTEMPTS {
            return Err(ServiceError::rejected(
                "auth.code_attempts",
                "Слишком много неверных попыток, запросите новый код",
            ));
        }
        if !same(&row.code_hash, &token_hash(&format!("{email}:{code}"))) {
            sqlx::query("UPDATE client_login_requests SET attempts = attempts + 1 WHERE id = $1")
                .bind(row.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            return Err(code_invalid());
        }
        if let Some(strike) = strike {
            self.services.limits().release(&mut tx, strike).await?;
        }
        let found = tx
            .clients()
            .find_by_identity(IdentityKind::Email, email)
            .await?;
        if let Some(client) = &found
            && !client.is_blocked()
            && linked_without_code(&mut tx, client.id, email).await?
        {
            return refuse_unconfirmed(tx, row.id).await.map(Ok);
        }
        let (client_id, created) = match found {
            Some(client) if client.is_blocked() => {
                tx.commit().await?;
                return Err(ServiceError::Forbidden);
            }
            Some(client) => (client.id, false),
            None => {
                let admission = match signups::admit(&mut tx, meta.ip.as_deref(), human).await? {
                    Ok(admission) => admission,
                    Err(refusal) => {
                        tx.rollback().await?;
                        return Ok(Err(refusal));
                    }
                };
                let client = tx
                    .clients()
                    .create(&NewClient {
                        locale: self.services.settings().languages.pick(lang::hint()),
                        registration_source: RegistrationSource::Web,
                        note: None,
                    })
                    .await?;
                tx.clients()
                    .add_identity(
                        client.id,
                        &NewIdentity::new(IdentityKind::Email, email, true)?,
                    )
                    .await?;
                signups::record(&mut tx, admission, client.id, Via::Code).await?;
                tx.events()
                    .push(&DomainEvent::ClientRegistered {
                        client_id: client.id,
                        source: RegistrationSource::Web,
                        start_param: from.start_param(),
                    })
                    .await?;
                (client.id, true)
            }
        };
        sqlx::query(
            "UPDATE client_login_requests SET status = 'consumed', client_id = $2, confirmed_at = $3, consumed_at = $3 \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(client_id.0)
        .bind(now)
        .execute(tx.conn())
        .await?;
        if !created {
            let unproven = email_unproven(&mut tx, client_id, email).await?;
            crate::oauth::confirm_email(&mut tx, client_id, email).await?;
            if unproven {
                drop_unproven_access(&mut tx, client_id).await?;
            }
        }
        let login = cabinet_session(&mut tx, client_id, now, meta, created).await?;
        tx.commit().await?;
        Ok(Ok(login))
    }

    pub async fn email_reset_start(
        &self,
        email: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<EmailLoginStart> {
        self.site_open()?;
        self.email_by_password()?;
        let mail = self.services.mail();
        if !mail.available() {
            return Err(ServiceError::rejected(
                "auth.reset_unavailable",
                "Восстановить пароль по почте нельзя: напишите в поддержку",
            ));
        }
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(limits.email_start(), meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_START_ADDRESS, &email).await?;
        limits
            .hit_wide(limits.email_start_wide(), meta.ip.as_deref())
            .await?;
        limits
            .letters_sent(RESET_LETTERS, mail.codes_per_hour())
            .await?;
        let known = {
            let mut conn = self.services.store().conn().await?;
            conn.clients()
                .find_by_identity(IdentityKind::Email, &email)
                .await?
                .is_some_and(|client| !client.is_blocked())
        };
        let (code, start) = self.issue_code(METHOD_RESET, &email, meta).await?;
        if known {
            self.services
                .letters()
                .send(LetterId::ResetCode, &start.email, code_values(code))
                .await?;
        }
        Ok(start)
    }

    pub async fn email_reset_confirm(
        &self,
        email: &str,
        code: &str,
        password: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<CabinetLogin> {
        self.site_open()?;
        self.email_by_password()?;
        let email = normalize_identity(IdentityKind::Email, email)?;
        let limits = self.services.limits();
        limits
            .hit_ip(rules::EMAIL_VERIFY_IP, meta.ip.as_deref())
            .await?;
        limits.hit(rules::EMAIL_VERIFY_ADDRESS, &email).await?;
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        if code.len() != EMAIL_CODE_DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(code_invalid());
        }
        let strike = code_strike(&limits, &email).await?;
        let hash = hash_password_async(password).await?;
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let row: Option<LoginRow> = sqlx::query_as(&format!(
            "SELECT {LOGIN_COLUMNS} FROM client_login_requests \
             WHERE method = 'reset' AND email = $1 AND status = 'pending' ORDER BY id DESC LIMIT 1 FOR UPDATE"
        ))
        .bind(&email)
        .fetch_optional(tx.conn())
        .await?;
        let Some(row) = row.filter(|r| r.expires_at > now) else {
            return Err(code_invalid());
        };
        if row.attempts >= EMAIL_CODE_MAX_ATTEMPTS {
            return Err(ServiceError::rejected(
                "auth.code_attempts",
                "Слишком много неверных попыток, запросите новый код",
            ));
        }
        if !same(&row.code_hash, &token_hash(&format!("{email}:{code}"))) {
            sqlx::query("UPDATE client_login_requests SET attempts = attempts + 1 WHERE id = $1")
                .bind(row.id)
                .execute(tx.conn())
                .await?;
            tx.commit().await?;
            return Err(code_invalid());
        }
        limits.release(&mut tx, &strike).await?;
        let found = tx
            .clients()
            .find_by_identity(IdentityKind::Email, &email)
            .await?;
        let client = match found {
            Some(client) if client.is_blocked() => {
                tx.commit().await?;
                return Err(ServiceError::Forbidden);
            }
            Some(client) => client,
            None => {
                tx.commit().await?;
                return Err(code_invalid());
            }
        };
        if linked_without_code(&mut tx, client.id, &email).await? {
            return refuse_unconfirmed(tx, row.id).await;
        }
        store_password(&mut tx, client.id, &hash, false).await?;
        let revoked = tx
            .sessions()
            .revoke_client_channel(client.id, SessionChannel::Cabinet)
            .await?;
        sqlx::query(
            "UPDATE client_login_requests SET status = 'consumed', client_id = $2, confirmed_at = $3, consumed_at = $3 \
             WHERE id = $1",
        )
        .bind(row.id)
        .bind(client.id.0)
        .bind(now)
        .execute(tx.conn())
        .await?;
        tx.audit()
            .record(&Actor::Client(client.id).audit(
                "client.password_reset",
                "client",
                Some(client.id.0),
                serde_json::json!({ "revoked_sessions": revoked }),
            ))
            .await?;
        crate::oauth::confirm_email(&mut tx, client.id, &email).await?;
        let login = cabinet_session(&mut tx, client.id, now, meta, false).await?;
        tx.commit().await?;
        Ok(login)
    }

    pub async fn has_password(&self, client_id: ClientId) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        let found: Option<i64> =
            sqlx::query_scalar("SELECT client_id FROM client_passwords WHERE client_id = $1")
                .bind(client_id.0)
                .fetch_optional(conn.conn())
                .await?;
        Ok(found.is_some())
    }

    pub async fn change_password(
        &self,
        principal: &ClientPrincipal,
        current: Option<&str>,
        password: &str,
    ) -> ServiceResult<()> {
        let client_id = principal.client_id;
        self.services
            .limits()
            .hit(rules::CLIENT_SECURITY, &client_id.to_string())
            .await?;
        let existing: Option<(String, bool)> = {
            let mut conn = self.services.store().conn().await?;
            let email = conn
                .clients()
                .identities(client_id)
                .await?
                .into_iter()
                .any(|i| i.kind == IdentityKind::Email);
            if !email {
                return Err(ServiceError::rejected(
                    "auth.no_email",
                    "К аккаунту не привязана почта: пароль для входа на сайт не нужен",
                ));
            }
            sqlx::query_as(
                "SELECT password_hash, must_change FROM client_passwords WHERE client_id = $1",
            )
            .bind(client_id.0)
            .fetch_optional(conn.conn())
            .await?
        };
        match existing {
            Some((issued, true)) => {
                if check_password(password, Some(issued)).await? {
                    return Err(ServiceError::rejected(
                        "auth.same_password",
                        "Придумайте свой пароль: выданный администратором пароль одноразовый",
                    ));
                }
            }
            Some((stored, false)) => {
                if !check_password(current.unwrap_or_default(), Some(stored)).await? {
                    return Err(ServiceError::rejected(
                        "auth.wrong_password",
                        "Текущий пароль не подошёл",
                    ));
                }
            }
            None => {}
        }
        let hash = hash_password_async(password).await?;
        let mut tx = self.services.store().begin().await?;
        store_password(&mut tx, client_id, &hash, false).await?;
        if let Some(session) = principal.session_id {
            sqlx::query(
                "UPDATE client_sessions SET scope = 'full' WHERE id = $1 AND scope = 'password_change'",
            )
            .bind(session)
            .execute(tx.conn())
            .await?;
        }
        let revoked: u64 = match principal.session_id {
            Some(keep) => sqlx::query(
                "UPDATE client_sessions SET revoked_at = now() \
                 WHERE client_id = $1 AND channel = 'cabinet' AND revoked_at IS NULL AND id <> $2",
            )
            .bind(client_id.0)
            .bind(keep)
            .execute(tx.conn())
            .await?
            .rows_affected(),
            None => 0,
        };
        tx.audit()
            .record(&Actor::Client(client_id).audit(
                "client.password_changed",
                "client",
                Some(client_id.0),
                serde_json::json!({ "revoked_sessions": revoked }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn issue_client_password(
        &self,
        client_id: ClientId,
        actor: Actor,
    ) -> ServiceResult<String> {
        let password = crate::admins::temporary_password();
        let hash = hash_password_async(&password).await?;
        let mut tx = self.services.store().begin().await?;
        let client = tx.clients().lock(client_id).await?;
        let identities = tx.clients().identities(client_id).await?;
        let Some(email) = identities.iter().find(|i| i.kind == IdentityKind::Email) else {
            return Err(ServiceError::rejected(
                "client.no_email",
                "У клиента нет почты: на сайт он входит через Telegram",
            ));
        };
        let in_bot =
            !client.bot_unreachable && identities.iter().any(|i| i.kind == IdentityKind::Telegram);
        store_password(&mut tx, client_id, &hash, true).await?;
        let revoked = tx
            .sessions()
            .revoke_client_channel(client_id, SessionChannel::Cabinet)
            .await?;
        tx.jobs()
            .enqueue(
                &JobPayload::Notify {
                    client_id,
                    template: sn_modules::catalog::oauth::TEMPLATE_PASSWORD_ISSUED.into(),
                    params: serde_json::json!({}),
                },
                None,
            )
            .await?;
        tx.audit()
            .record(&actor.audit(
                "client.password_issued",
                "client",
                Some(client_id.0),
                serde_json::json!({ "revoked_sessions": revoked, "one_time": true }),
            ))
            .await?;
        tx.commit().await?;
        if in_bot && email.takes_letters() {
            let sent = self
                .services
                .letters()
                .send(LetterId::PasswordIssued, &email.value, Values::new())
                .await;
            if let Err(err) = sent {
                tracing::warn!(client = %client_id, error = %err, "письмо о выданном пароле не отправилось");
            }
        }
        Ok(password)
    }
}

pub const UNPROVEN_ACCESS_AUDIT: &str = "client.unproven_access_dropped";
pub const EMAIL_UNCONFIRMED: &str = "auth.email_unconfirmed";

pub(crate) async fn code_strike(limits: &RateLimits<'_>, email: &str) -> ServiceResult<Strike> {
    limits
        .strike(
            rules::EMAIL_CODE_FAILURES_DAY,
            email,
            "Слишком много неверных кодов для этой почты.",
        )
        .await
}

async fn linked_without_code(tx: &mut Tx, client_id: ClientId, email: &str) -> ServiceResult<bool> {
    let identities = tx.clients().identities(client_id).await?;
    Ok(identities
        .iter()
        .any(|i| i.value == email && i.linked_without_code())
        && identities.iter().any(|i| i.kind != IdentityKind::Email))
}

async fn refuse_unconfirmed<T>(mut tx: Tx, request_id: i64) -> ServiceResult<T> {
    sqlx::query("UPDATE client_login_requests SET status = 'rejected' WHERE id = $1")
        .bind(request_id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Err(ServiceError::rejected(
        EMAIL_UNCONFIRMED,
        "Эта почта указана в аккаунте без подтверждения, поэтому код из письма его не откроет. Если аккаунт ваш, войдите через Telegram и подтвердите почту: в мини-приложении «Профиль» → «Почта», в кабинете «Безопасность» → «Почта». Если не ваш, напишите в поддержку.",
    ))
}

async fn email_unproven(tx: &mut Tx, client_id: ClientId, email: &str) -> ServiceResult<bool> {
    let verified: Option<bool> = sqlx::query_scalar(
        "SELECT verified FROM client_identities WHERE client_id = $1 AND kind = 'email' AND value = $2",
    )
    .bind(client_id.0)
    .bind(email)
    .fetch_optional(tx.conn())
    .await?;
    Ok(verified == Some(false))
}

async fn drop_unproven_access(tx: &mut Tx, client_id: ClientId) -> ServiceResult<()> {
    let revoked = tx
        .sessions()
        .revoke_client_channel(client_id, SessionChannel::Cabinet)
        .await?;
    let password_removed = sqlx::query("DELETE FROM client_passwords WHERE client_id = $1")
        .bind(client_id.0)
        .execute(tx.conn())
        .await?
        .rows_affected()
        > 0;
    if revoked == 0 && !password_removed {
        return Ok(());
    }
    tx.audit()
        .record(&Actor::Client(client_id).audit(
            UNPROVEN_ACCESS_AUDIT,
            "client",
            Some(client_id.0),
            serde_json::json!({
                "revoked_sessions": revoked,
                "password_removed": password_removed,
            }),
        ))
        .await?;
    tracing::warn!(client = %client_id, revoked, password_removed, "почту впервые подтвердили кодом: прежние сессии кабинета закрыты, пароль, заданный до подтверждения, удалён");
    Ok(())
}

async fn store_password(
    tx: &mut Tx,
    client_id: ClientId,
    hash: &str,
    must_change: bool,
) -> ServiceResult<()> {
    sqlx::query(
        "INSERT INTO client_passwords (client_id, password_hash, must_change) VALUES ($1, $2, $3) \
         ON CONFLICT (client_id) DO UPDATE SET password_hash = EXCLUDED.password_hash, \
         must_change = EXCLUDED.must_change, updated_at = now()",
    )
    .bind(client_id.0)
    .bind(hash)
    .bind(must_change)
    .execute(tx.conn())
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn устройство_входа_описывается_по_user_agent() {
        assert_eq!(
            describe_user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"
            )
            .as_deref(),
            Some("Chrome, Windows")
        );
        assert_eq!(
            describe_user_agent(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 Version/18.1 Mobile/15E148 Safari/604.1"
            )
            .as_deref(),
            Some("Safari, iOS")
        );
        assert_eq!(describe_user_agent("curl/8.0"), None);
    }

    #[test]
    fn код_из_ссылки_принимается_с_префиксом_и_без() {
        let code = "0123456789abcdef0123456789ABCDEF";
        assert_eq!(
            link_code(&format!("auth_{code}")).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            link_code(code).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(link_code("auth_short"), None);
        assert_eq!(link_code("auth_zz23456789abcdef0123456789abcdef"), None);
    }

    #[test]
    fn текст_подтверждения_называет_аккаунт_и_место() {
        let at = DateTime::parse_from_rfc3339("2026-11-16T14:52:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let prompt = TelegramLoginPrompt {
            account: account_title(&TelegramProfile {
                id: 1,
                username: Some("morozov_a".into()),
                first_name: Some("Алексей".into()),
                last_name: None,
                language_code: None,
                is_premium: false,
            }),
            ip: Some("203.0.113.7".into()),
            device: Some("Chrome, Windows".into()),
            requested_at: at,
            expires_at: at + Duration::minutes(5),
        };
        let text = prompt.text();
        assert!(text.contains("Аккаунт: Алексей (@morozov_a)"));
        assert!(text.contains("Откуда: 203.0.113.7, Chrome, Windows"));
        assert!(text.contains("14:52"));
    }
}
