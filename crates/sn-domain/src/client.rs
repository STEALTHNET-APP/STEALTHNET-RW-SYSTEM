use crate::DomainError;
use crate::ids::{ClientId, TelegramId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientStatus {
    #[default]
    Active,
    Blocked,
}

impl ClientStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ClientStatus::Active => "active",
            ClientStatus::Blocked => "blocked",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "active" => Ok(ClientStatus::Active),
            "blocked" => Ok(ClientStatus::Blocked),
            other => Err(DomainError::unknown("client_status", other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locale {
    #[default]
    Ru,
    En,
}

impl Locale {
    pub const fn as_str(self) -> &'static str {
        match self {
            Locale::Ru => "ru",
            Locale::En => "en",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "ru" => Ok(Locale::Ru),
            "en" => Ok(Locale::En),
            other => Err(DomainError::unknown("locale", other)),
        }
    }

    pub const ALL: [Locale; 2] = [Locale::Ru, Locale::En];

    pub fn from_tag(tag: &str) -> Option<Self> {
        let base = tag
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if base.len() < 2 || base.len() > 3 || !base.bytes().all(|b| b.is_ascii_lowercase()) {
            return None;
        }
        Some(match base.as_str() {
            "ru" | "uk" | "be" | "kk" => Locale::Ru,
            _ => Locale::En,
        })
    }

    pub const fn native_name(self) -> &'static str {
        match self {
            Locale::Ru => "Русский",
            Locale::En => "English",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Languages {
    #[serde(default)]
    pub english: bool,
    #[serde(default)]
    pub default: Locale,
}

impl Default for Languages {
    fn default() -> Self {
        Languages {
            english: false,
            default: Locale::Ru,
        }
    }
}

impl Languages {
    pub const fn allows(self, locale: Locale) -> bool {
        match locale {
            Locale::Ru => true,
            Locale::En => self.english,
        }
    }

    pub fn enabled(self) -> Vec<Locale> {
        Locale::ALL
            .into_iter()
            .filter(|l| self.allows(*l))
            .collect()
    }

    pub const fn several(self) -> bool {
        self.english
    }

    pub const fn fallback(self) -> Locale {
        if self.allows(self.default) {
            self.default
        } else {
            Locale::Ru
        }
    }

    pub const fn effective(self, locale: Locale) -> Locale {
        if self.allows(locale) {
            locale
        } else {
            self.fallback()
        }
    }

    pub fn pick(self, wanted: Option<Locale>) -> Locale {
        wanted
            .filter(|l| self.allows(*l))
            .unwrap_or_else(|| self.fallback())
    }

    pub fn detect(self, tag: Option<&str>) -> Locale {
        self.pick(tag.and_then(Locale::from_tag))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationSource {
    Bot,
    Miniapp,
    Web,
    Admin,
    Import,
    Migration,
}

impl RegistrationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            RegistrationSource::Bot => "bot",
            RegistrationSource::Miniapp => "miniapp",
            RegistrationSource::Web => "web",
            RegistrationSource::Admin => "admin",
            RegistrationSource::Import => "import",
            RegistrationSource::Migration => "migration",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "bot" => Ok(RegistrationSource::Bot),
            "miniapp" => Ok(RegistrationSource::Miniapp),
            "web" => Ok(RegistrationSource::Web),
            "admin" => Ok(RegistrationSource::Admin),
            "import" => Ok(RegistrationSource::Import),
            "migration" => Ok(RegistrationSource::Migration),
            other => Err(DomainError::unknown("registration_source", other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub id: ClientId,
    pub public_id: Uuid,
    pub status: ClientStatus,
    pub block_reason: Option<String>,
    pub locale: Locale,
    pub note: Option<String>,
    pub registration_source: RegistrationSource,
    pub bot_unreachable: bool,
    pub created_at: DateTime<Utc>,
    pub blocked_at: Option<DateTime<Utc>>,
}

impl Client {
    pub const fn is_blocked(&self) -> bool {
        matches!(self.status, ClientStatus::Blocked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Telegram,
    Email,
    Google,
    Apple,
}

impl IdentityKind {
    pub const ALL: [IdentityKind; 4] = [
        IdentityKind::Telegram,
        IdentityKind::Email,
        IdentityKind::Google,
        IdentityKind::Apple,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            IdentityKind::Telegram => "telegram",
            IdentityKind::Email => "email",
            IdentityKind::Google => "google",
            IdentityKind::Apple => "apple",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DomainError> {
        IdentityKind::ALL
            .into_iter()
            .find(|k| k.as_str() == value)
            .ok_or_else(|| DomainError::unknown("identity_kind", value))
    }
}

pub const LINKED_WITHOUT_CODE: &str = "no_code";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub kind: IdentityKind,
    pub value: String,
    pub verified: bool,
    pub meta: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl Identity {
    pub fn linked_without_code(&self) -> bool {
        self.kind == IdentityKind::Email
            && !self.verified
            && self.meta.get(LINKED_WITHOUT_CODE).is_some()
    }

    pub fn takes_letters(&self) -> bool {
        self.kind == IdentityKind::Email && !self.linked_without_code()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewIdentity {
    pub kind: IdentityKind,
    pub value: String,
    pub verified: bool,
    pub meta: serde_json::Value,
}

pub fn proven_identities(identities: Vec<Identity>) -> Vec<(IdentityKind, String)> {
    identities
        .into_iter()
        .filter(|i| !i.linked_without_code())
        .map(|i| (i.kind, i.value))
        .collect()
}

impl NewIdentity {
    pub fn new(kind: IdentityKind, raw: &str, verified: bool) -> Result<Self, DomainError> {
        Ok(NewIdentity {
            kind,
            value: normalize_identity(kind, raw)?,
            verified,
            meta: serde_json::Value::Object(Default::default()),
        })
    }

    pub fn email_without_code(raw: &str) -> Result<Self, DomainError> {
        Ok(NewIdentity {
            meta: serde_json::json!({ LINKED_WITHOUT_CODE: true }),
            ..NewIdentity::new(IdentityKind::Email, raw, false)?
        })
    }

    pub fn telegram(profile: &TelegramProfile) -> Self {
        NewIdentity {
            kind: IdentityKind::Telegram,
            value: profile.id.to_string(),
            verified: true,
            meta: serde_json::json!({
                "username": profile.username,
                "first_name": profile.first_name,
                "last_name": profile.last_name,
                "language_code": profile.language_code,
                "is_premium": profile.is_premium,
            }),
        }
    }
}

const UNDELIVERABLE_DOMAINS: [&str; 5] = ["local", "localhost", "invalid", "internal", "home.arpa"];

pub fn deliverable_email(email: &str) -> bool {
    let Some((name, domain)) = email.trim().rsplit_once('@') else {
        return false;
    };
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    !name.is_empty()
        && domain.contains('.')
        && !UNDELIVERABLE_DOMAINS
            .iter()
            .any(|reserved| domain == *reserved || domain.ends_with(&format!(".{reserved}")))
}

pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((name, domain)) => {
            let first: String = name.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        None => "***".into(),
    }
}

pub fn normalize_identity(kind: IdentityKind, raw: &str) -> Result<String, DomainError> {
    let value = raw.trim();
    match kind {
        IdentityKind::Telegram => {
            let id: i64 = value
                .parse()
                .map_err(|_| DomainError::invalid("telegram_id", "ожидается число"))?;
            if id <= 0 {
                return Err(DomainError::invalid(
                    "telegram_id",
                    "ожидается положительное число",
                ));
            }
            Ok(id.to_string())
        }
        IdentityKind::Email => {
            let lower = value.to_lowercase();
            let valid = lower.len() <= 254
                && lower.split_once('@').is_some_and(|(local, domain)| {
                    !local.is_empty()
                        && domain.contains('.')
                        && !domain.starts_with('.')
                        && !domain.ends_with('.')
                        && !lower.contains(char::is_whitespace)
                        && lower.matches('@').count() == 1
                });
            if valid {
                Ok(lower)
            } else {
                Err(DomainError::invalid("email", "адрес почты указан неверно"))
            }
        }
        IdentityKind::Google | IdentityKind::Apple => {
            if value.is_empty() || value.len() > 255 {
                Err(DomainError::invalid(
                    "subject",
                    "пустой или слишком длинный идентификатор",
                ))
            } else {
                Ok(value.to_string())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramProfile {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub language_code: Option<String>,
    pub is_premium: bool,
}

impl TelegramProfile {
    pub const fn telegram_id(&self) -> TelegramId {
        TelegramId(self.id)
    }

    pub fn display_name(&self) -> String {
        let full = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !full.is_empty() {
            full
        } else if let Some(username) = &self.username {
            format!("@{username}")
        } else {
            self.id.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn служебная_почта_не_считается_настоящей() {
        for email in [
            "anna@example.com",
            "Client-17@3apes.pro",
            "a@mail.ru",
            "x@sub.domain.org.",
        ] {
            assert!(deliverable_email(email), "{email}");
        }
        for email in [
            "tg_5250903268@telegram.local",
            "tg_1@TELEGRAM.LOCAL",
            "root@localhost",
            "a@box.localhost",
            "a@x.invalid",
            "a@corp.internal",
            "a@router.home.arpa",
            "a@nodot",
            "@example.com",
            "no-at-sign",
        ] {
            assert!(!deliverable_email(email), "{email}");
        }
    }

    #[test]
    fn почта_без_кода_только_логин_пока_не_подтверждена() {
        let identity = |new: NewIdentity| Identity {
            kind: new.kind,
            value: new.value,
            verified: new.verified,
            meta: new.meta,
            created_at: Utc::now(),
        };
        let linked = NewIdentity::email_without_code(" Alex@Example.com ").unwrap();
        assert_eq!(linked.value, "alex@example.com");
        assert!(NewIdentity::email_without_code("не почта").is_err());
        let mut linked = identity(linked);
        assert!(linked.linked_without_code() && !linked.takes_letters());
        let typed =
            identity(NewIdentity::new(IdentityKind::Email, "b@example.com", false).unwrap());
        assert!(!typed.linked_without_code() && typed.takes_letters());
        let telegram = identity(NewIdentity::new(IdentityKind::Telegram, "77", true).unwrap());
        assert!(!telegram.linked_without_code() && !telegram.takes_letters());
        assert_eq!(
            proven_identities(vec![telegram.clone(), linked.clone(), typed]),
            [
                (IdentityKind::Telegram, "77".to_string()),
                (IdentityKind::Email, "b@example.com".to_string()),
            ]
        );
        linked.verified = true;
        assert!(!linked.linked_without_code() && linked.takes_letters());
        assert_eq!(proven_identities(vec![linked]).len(), 1);
    }

    #[test]
    fn почта_приводится_к_нижнему_регистру() {
        assert_eq!(
            normalize_identity(IdentityKind::Email, "  Alex@Example.COM ").unwrap(),
            "alex@example.com"
        );
        for bad in [
            "alex",
            "@example.com",
            "alex@example",
            "a b@example.com",
            "a@@b.com",
        ] {
            assert!(
                normalize_identity(IdentityKind::Email, bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn telegram_id_только_положительное_число() {
        assert_eq!(
            normalize_identity(IdentityKind::Telegram, "5093118422").unwrap(),
            "5093118422"
        );
        assert!(normalize_identity(IdentityKind::Telegram, "-100").is_err());
        assert!(normalize_identity(IdentityKind::Telegram, "abc").is_err());
    }

    #[test]
    fn язык_по_коду_из_telegram_и_браузера() {
        assert_eq!(Locale::from_tag("ru"), Some(Locale::Ru));
        assert_eq!(Locale::from_tag("uk-UA"), Some(Locale::Ru));
        assert_eq!(Locale::from_tag("en-US"), Some(Locale::En));
        assert_eq!(Locale::from_tag("de"), Some(Locale::En));
        assert_eq!(Locale::from_tag("pt_BR"), Some(Locale::En));
        assert_eq!(Locale::from_tag(""), None);
        assert_eq!(Locale::from_tag("x"), None);
        assert_eq!(Locale::from_tag("12"), None);
    }

    #[test]
    fn без_английского_все_новые_клиенты_на_русском() {
        let only_ru = Languages::default();
        assert_eq!(only_ru.enabled(), vec![Locale::Ru]);
        assert_eq!(only_ru.detect(Some("en")), Locale::Ru);
        assert_eq!(only_ru.detect(None), Locale::Ru);
        assert_eq!(only_ru.pick(Some(Locale::En)), Locale::Ru);
        assert_eq!(only_ru.effective(Locale::En), Locale::Ru);
        let broken = Languages {
            english: false,
            default: Locale::En,
        };
        assert_eq!(broken.fallback(), Locale::Ru);
        assert_eq!(broken.detect(None), Locale::Ru);
    }

    #[test]
    fn с_английским_язык_берётся_из_telegram_а_без_кода_по_умолчанию() {
        let ru_first = Languages {
            english: true,
            default: Locale::Ru,
        };
        assert_eq!(ru_first.enabled(), vec![Locale::Ru, Locale::En]);
        assert_eq!(ru_first.detect(Some("en-GB")), Locale::En);
        assert_eq!(ru_first.detect(Some("es")), Locale::En);
        assert_eq!(ru_first.detect(Some("ru")), Locale::Ru);
        assert_eq!(ru_first.detect(None), Locale::Ru);
        assert_eq!(ru_first.detect(Some("")), Locale::Ru);
        let en_first = Languages {
            english: true,
            default: Locale::En,
        };
        assert_eq!(en_first.detect(None), Locale::En);
        assert_eq!(en_first.detect(Some("kk")), Locale::Ru);
        assert_eq!(en_first.pick(None), Locale::En);
        assert_eq!(en_first.effective(Locale::Ru), Locale::Ru);
    }
}
