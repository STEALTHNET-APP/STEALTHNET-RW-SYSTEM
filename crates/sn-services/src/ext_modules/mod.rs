pub mod address;
pub mod bot;
pub mod cards;
pub mod delivery;
pub mod inbound;
pub mod manifest;
pub mod package;
pub mod packages;
pub mod proxy;
pub mod rewards;
pub mod runtime;
pub mod signing;
pub mod store;
#[cfg(test)]
mod tests;

use crate::Actor;
use crate::api_keys::{SHOWN_PREFIX_LEN, new_token};
use crate::auth::token_hash;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::settings::{read_secret, write_secret};
use chrono::{DateTime, Duration, Utc};
use manifest::{
    Field, FieldKind, Manifest, Problem, event_info, permission_about, permission_title,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::access::{AdminPrincipal, Permission};
use sn_store::{Db, NewApiKey};
use store::{KeyInfo, KeyState, ModuleRow, Stats};

pub use delivery::Deliverer;

const JOURNAL_MAX: i64 = 100;
const PING_LEASE_SECONDS: i64 = 60;

pub fn secret_key(module: &str) -> String {
    format!("ext_module.{module}.signing")
}

fn setting_key(module: &str, field: &str) -> String {
    format!("ext_module.{module}.setting.{field}")
}

fn secrets_prefix(module: &str) -> String {
    format!("ext_module.{module}.")
}

fn rejected(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn missing() -> ServiceError {
    ServiceError::NotFound("внешний модуль")
}

fn installer(actor: &AdminPrincipal) -> ServiceResult<()> {
    if actor.api_key.is_some()
        || !actor.can(Permission::Modules)
        || !actor.can(Permission::Settings)
    {
        return Err(ServiceError::Forbidden);
    }
    Ok(())
}

fn manager(actor: &AdminPrincipal) -> ServiceResult<()> {
    if actor.api_key.is_some() || !actor.can(Permission::Modules) {
        return Err(ServiceError::Forbidden);
    }
    Ok(())
}

fn problem(path: &str, message: impl Into<String>) -> Problem {
    Problem {
        path: path.to_string(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Same,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventView {
    pub kind: String,
    pub title: &'static str,
    pub about: &'static str,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PermissionView {
    pub key: Permission,
    pub title: &'static str,
    pub about: &'static str,
    pub sensitive: bool,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageView {
    pub url: String,
    pub path: Option<String>,
    pub title: String,
    pub permission: Permission,
    pub permission_title: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientCardView {
    pub title: String,
    pub permission: Permission,
    pub permission_title: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BotView {
    pub button: String,
}

pub const INBOUND_PREFIX: &str = "/api/ext/";

pub fn inbound_address(module: &str) -> String {
    format!("{INBOUND_PREFIX}{module}/")
}

fn client_card_view(manifest: &Manifest) -> Option<ClientCardView> {
    manifest.client_card.as_ref().map(|card| ClientCardView {
        title: card.title.clone(),
        permission: card.permission,
        permission_title: permission_title(card.permission),
    })
}

fn bot_view(manifest: &Manifest) -> Option<BotView> {
    manifest.bot.as_ref().map(|bot| BotView {
        button: bot.button.clone(),
    })
}

fn inbound_view(manifest: &Manifest) -> Option<String> {
    manifest
        .run
        .as_ref()
        .and_then(|run| run.inbound_path.as_ref())
        .map(|_| inbound_address(&manifest.id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Address {
    Webhook,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Moved {
    pub address: Address,
    pub before: Option<String>,
    pub after: Option<String>,
}

fn page_url(manifest: &Manifest) -> Option<&str> {
    manifest.page.as_ref().map(|page| page.url.as_str())
}

fn moved_addresses(manifest: &Manifest, old: Option<&Manifest>) -> Vec<Moved> {
    let Some(old) = old else {
        return Vec::new();
    };
    let mut moved = Vec::new();
    if old.webhook != manifest.webhook {
        moved.push(Moved {
            address: Address::Webhook,
            before: old.webhook.clone(),
            after: manifest.webhook.clone(),
        });
    }
    if page_url(old) != page_url(manifest) {
        moved.push(Moved {
            address: Address::Page,
            before: page_url(old).map(str::to_string),
            after: page_url(manifest).map(str::to_string),
        });
    }
    moved
}

fn page_view(manifest: &Manifest) -> Option<PageView> {
    manifest.page.as_ref().map(|page| PageView {
        url: page.url.clone(),
        path: page.path.clone(),
        title: page.title.clone(),
        permission: page.permission,
        permission_title: permission_title(page.permission),
    })
}

fn event_view(kind: &str, change: Change) -> EventView {
    let info = event_info(kind);
    EventView {
        kind: kind.to_string(),
        title: info.map(|i| i.title).unwrap_or("Событие"),
        about: info.map(|i| i.about).unwrap_or_default(),
        change,
    }
}

fn permission_view(permission: Permission, change: Change) -> PermissionView {
    PermissionView {
        key: permission,
        title: permission_title(permission),
        about: permission_about(permission),
        sensitive: manifest::sensitive(permission),
        change,
    }
}

fn events_diff(new: &Manifest, old: Option<&Manifest>) -> Vec<EventView> {
    let mut out: Vec<EventView> = new
        .events
        .iter()
        .map(|kind| {
            let change = match old {
                Some(old) if !old.events.contains(kind) => Change::Added,
                _ => Change::Same,
            };
            event_view(kind, change)
        })
        .collect();
    if let Some(old) = old {
        out.extend(
            old.events
                .iter()
                .filter(|kind| !new.events.contains(kind))
                .map(|kind| event_view(kind, Change::Removed)),
        );
    }
    out
}

fn permissions_diff(new: &Manifest, old: Option<&Manifest>) -> Vec<PermissionView> {
    let mut out: Vec<PermissionView> = new
        .permissions
        .iter()
        .map(|p| {
            let change = match old {
                Some(old) if !old.permissions.contains(p) => Change::Added,
                _ => Change::Same,
            };
            permission_view(*p, change)
        })
        .collect();
    if let Some(old) = old {
        out.extend(
            old.permissions
                .iter()
                .filter(|p| !new.permissions.contains(p))
                .map(|p| permission_view(*p, Change::Removed)),
        );
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledVersion {
    pub version: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    pub site: Option<String>,
    pub webhook: Option<String>,
    pub page: Option<PageView>,
    pub client_card: Option<ClientCardView>,
    pub bot: Option<BotView>,
    pub inbound: Option<String>,
    pub settings: Vec<String>,
    pub events: Vec<EventView>,
    pub permissions: Vec<PermissionView>,
    pub sensitive: bool,
    pub installed: Option<InstalledVersion>,
    pub moved: Vec<Moved>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewResult {
    pub errors: Vec<Problem>,
    pub preview: Option<Preview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstallRequest {
    pub manifest: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub update: Option<String>,
    #[serde(default)]
    pub webhook: Option<String>,
    #[serde(default)]
    pub page: Option<String>,
}

impl InstallRequest {
    fn confirms(&self, moved: &Moved) -> bool {
        let confirmed = match moved.address {
            Address::Webhook => self.webhook.as_deref(),
            Address::Page => self.page.as_deref(),
        };
        confirmed == moved.after.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Credentials {
    pub api_key: String,
    pub signing_secret: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstallResult {
    pub module: Card,
    pub created: bool,
    pub credentials: Option<Credentials>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub has_settings: bool,
    pub page: Option<PageView>,
    pub warning: Option<String>,
    pub managed: bool,
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    pub site: Option<String>,
    pub webhook: Option<String>,
    pub enabled: bool,
    pub events: Vec<EventView>,
    pub permissions: Vec<PermissionView>,
    pub page: Option<PageView>,
    pub client_card: Option<ClientCardView>,
    pub bot: Option<BotView>,
    pub inbound: Option<String>,
    pub has_settings: bool,
    pub key: KeyInfo,
    pub installed_by: String,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub stats: Stats,
    pub warning: Option<String>,
    pub managed: bool,
    pub takes_events: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigView {
    pub module: String,
    pub schema: Vec<Field>,
    pub config: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigSaved {
    #[serde(flatten)]
    pub view: ConfigView,
    pub event_queued: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ConfigUpdate {
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptView {
    pub id: i64,
    pub delivery_id: i64,
    pub kind: String,
    pub title: &'static str,
    pub event_id: Option<i64>,
    pub attempt: i32,
    pub at: DateTime<Utc>,
    pub http_status: Option<i32>,
    pub duration_ms: i32,
    pub error: Option<String>,
    pub response: Option<String>,
    pub status: String,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub can_retry: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Journal {
    pub attempts: Vec<AttemptView>,
    pub stats: Stats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PingResult {
    pub ok: bool,
    pub http_status: Option<u16>,
    pub duration_ms: u32,
    pub error: Option<String>,
    pub response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageToken {
    pub url: String,
    pub origin: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub proxied: bool,
    pub title: String,
    pub module: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuPage {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelfView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub events: Vec<String>,
    pub permissions: Vec<Permission>,
    pub settings: Map<String, Value>,
}

pub fn kind_title(kind: &str) -> &'static str {
    match kind {
        delivery::PING => "Проверка связи",
        delivery::SETTINGS_CHANGED => "Настройки изменены",
        other => event_info(other).map(|i| i.title).unwrap_or("Событие"),
    }
}

fn loopback_warning() -> String {
    format!(
        "Адрес модуля на этом же сервере закрыт: если модуль работает здесь, добавьте {}=1 в deploy/.env и перезапустите панель",
        address::LOOPBACK_ENV
    )
}

fn local_addresses(manifest: &Manifest) -> bool {
    manifest.webhook.as_deref().is_some_and(address::local)
        || page_url(manifest).is_some_and(address::local)
}

fn warning(row: &ModuleRow, key: &KeyInfo, stats: &Stats, loopback: bool) -> Option<String> {
    if !row.enabled {
        return None;
    }
    let key_text = match key.state {
        KeyState::Active => None,
        KeyState::Missing | KeyState::Revoked => {
            Some("Ключ API отозван: выпустите новый в карточке модуля")
        }
        KeyState::Expired => Some("Срок ключа API истёк: выпустите новый в карточке модуля"),
        KeyState::OwnerDisabled => Some(
            "Ключ API не работает: администратор, который его выпустил, отключён. Выпустите новый",
        ),
    };
    if let Some(text) = key_text {
        return Some(text.to_string());
    }
    if !loopback && local_addresses(&row.manifest) {
        return Some(loopback_warning());
    }
    if stats.failed > 0 {
        return Some(format!(
            "Не доставлено событий: {}. Подробности — в журнале",
            stats.failed
        ));
    }
    if stats.last_ok == Some(false) {
        return Some("Последняя доставка не прошла, панель повторит её".into());
    }
    None
}

fn sorted<T: Ord + Clone>(items: &[T]) -> Vec<T> {
    let mut items = items.to_vec();
    items.sort();
    items.dedup();
    items
}

fn key_name(manifest: &Manifest) -> String {
    format!("Модуль «{}»", manifest.name)
        .chars()
        .take(80)
        .collect()
}

fn same_origin(a: &str, b: &str) -> bool {
    match (reqwest::Url::parse(a), reqwest::Url::parse(b.trim())) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

fn carry_config(row: &ModuleRow, fields: &[Field]) -> (Map<String, Value>, Vec<String>) {
    let mut config = Map::new();
    for (key, value) in &row.config {
        let Some(field) = fields.iter().find(|f| f.key == *key && !f.is_secret()) else {
            continue;
        };
        let old = row.manifest.settings.iter().find(|f| f.key == *key);
        if old.is_none_or(|old| old.kind.name() != field.kind.name()) {
            continue;
        }
        if check_value(field, value).is_ok_and(|v| v == *value) {
            config.insert(key.clone(), value.clone());
        }
    }
    let dropped = row
        .manifest
        .settings
        .iter()
        .filter(|old| old.is_secret())
        .filter(|old| !fields.iter().any(|f| f.key == old.key && f.is_secret()))
        .map(|old| old.key.clone())
        .collect();
    (config, dropped)
}

fn value_error(field: &Field, reason: &str) -> ServiceError {
    rejected(
        "ext_modules.settings",
        format!("поле «{}»: {reason}", field.label),
    )
}

fn check_value(field: &Field, value: &Value) -> ServiceResult<Value> {
    match &field.kind {
        FieldKind::Flag => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if s.trim() == "true" => Ok(Value::Bool(true)),
            Value::String(s) if s.trim() == "false" => Ok(Value::Bool(false)),
            _ => Err(value_error(field, "ожидается да или нет")),
        },
        FieldKind::Number => {
            let parsed = match value {
                Value::Number(n) => n.as_f64().filter(|f| f.is_finite()).map(|_| n.clone()),
                Value::String(s) => manifest::number(s),
                _ => None,
            };
            parsed
                .map(Value::Number)
                .ok_or_else(|| value_error(field, "ожидается число"))
        }
        FieldKind::Secret => Err(value_error(field, "секрет хранится отдельно")),
        FieldKind::Text | FieldKind::Url | FieldKind::Select { .. } => {
            let raw = match value {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(value_error(field, "ожидается текст")),
            };
            match &field.kind {
                FieldKind::Url if !address::setting_url(&raw) => {
                    Err(value_error(field, "ожидается адрес https://…"))
                }
                FieldKind::Select { options } if !options.contains(&raw) => {
                    Err(value_error(field, "выберите значение из списка"))
                }
                FieldKind::Text => {
                    let max = if field.large {
                        manifest::LARGE_TEXT_MAX
                    } else {
                        manifest::TEXT_MAX
                    };
                    if raw.chars().count() > max {
                        Err(value_error(field, &format!("не длиннее {max} символов")))
                    } else {
                        Ok(Value::String(raw))
                    }
                }
                _ => Ok(Value::String(raw)),
            }
        }
    }
}

fn typed_default(field: &Field) -> Value {
    let Some(raw) = field.default.as_deref() else {
        return Value::Null;
    };
    match field.kind {
        FieldKind::Flag => Value::Bool(raw == "true"),
        FieldKind::Number => manifest::number(raw)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        _ => Value::String(raw.to_string()),
    }
}

fn blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

pub(crate) fn build_preview(manifest: Manifest, existing: Option<&ModuleRow>) -> Preview {
    let old = existing.map(|row| &row.manifest);
    let permissions = permissions_diff(&manifest, old);
    Preview {
        moved: moved_addresses(&manifest, old),
        sensitive: permissions
            .iter()
            .any(|p| p.sensitive && p.change != Change::Removed),
        events: events_diff(&manifest, old),
        permissions,
        page: page_view(&manifest),
        client_card: client_card_view(&manifest),
        bot: bot_view(&manifest),
        inbound: inbound_view(&manifest),
        settings: manifest.settings.iter().map(|f| f.label.clone()).collect(),
        installed: existing.map(|row| InstalledVersion {
            version: row.manifest.version.clone(),
            enabled: row.enabled,
        }),
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        author: manifest.author,
        description: manifest.description,
        site: manifest.site,
        webhook: manifest.webhook,
    }
}

pub(crate) fn admin_claims(
    actor: &AdminPrincipal,
    email: String,
    module: &str,
    now: DateTime<Utc>,
) -> (signing::PageClaims, DateTime<Utc>) {
    let expires_at = now + Duration::seconds(signing::PAGE_TOKEN_SECONDS);
    let claims = signing::PageClaims {
        iss: signing::ISSUER.into(),
        aud: signing::audience(module),
        sub: actor.admin_id.0.to_string(),
        iat: now.timestamp(),
        exp: expires_at.timestamp(),
        jti: signing::nonce(),
        module: module.to_string(),
        admin_id: actor.admin_id.0,
        email,
        role: actor.role,
        permissions: Permission::ALL
            .into_iter()
            .filter(|p| actor.can(*p))
            .collect(),
    };
    (claims, expires_at)
}

pub struct ExtModulesOps<'a> {
    services: &'a Services,
}

impl<'a> ExtModulesOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        ExtModulesOps { services }
    }

    fn loopback(&self) -> bool {
        self.services.env().ext_modules_loopback
    }

    fn local_problems(&self, manifest: &Manifest) -> Vec<Problem> {
        if self.loopback() {
            return Vec::new();
        }
        let mut problems = Vec::new();
        if manifest.webhook.as_deref().is_some_and(address::local) {
            problems.push(problem("webhook", address::loopback_closed()));
        }
        if page_url(manifest).is_some_and(address::local) {
            problems.push(problem("page.url", address::loopback_closed()));
        }
        problems
    }

    fn panel_origin_problems(&self, manifest: &Manifest) -> Vec<Problem> {
        let settings = self.services.settings();
        let Some(panel) = settings.public_url.as_deref() else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        if let Some(page) = &manifest.page
            && same_origin(&page.url, panel)
        {
            problems.push(problem(
                "page.url",
                "страница модуля не может лежать на адресе самой панели: откройте её на отдельном домене или порту",
            ));
        }
        if let Some(webhook) = &manifest.webhook
            && same_origin(webhook, panel)
        {
            problems.push(problem(
                "webhook",
                "события нельзя слать на адрес самой панели: у модуля должен быть свой адрес",
            ));
        }
        problems
    }

    fn rights_problems(&self, actor: &AdminPrincipal, manifest: &Manifest) -> Vec<Problem> {
        manifest
            .permissions
            .iter()
            .filter(|p| !actor.can(**p))
            .map(|p| {
                problem(
                    "permissions",
                    format!(
                        "у вас нет права «{}», поэтому модуль не может его получить",
                        permission_title(*p)
                    ),
                )
            })
            .collect()
    }

    pub async fn preview(
        &self,
        actor: &AdminPrincipal,
        source: &str,
        update: Option<&str>,
    ) -> ServiceResult<PreviewResult> {
        installer(actor)?;
        let manifest = match manifest::parse(source) {
            Ok(manifest) => manifest,
            Err(errors) => {
                return Ok(PreviewResult {
                    errors,
                    preview: None,
                });
            }
        };
        let mut errors = Vec::new();
        if let Some(expected) = update
            && expected != manifest.id
        {
            errors.push(problem(
                "id",
                format!(
                    "в файле описан другой модуль: «{}», а обновляется «{expected}»",
                    manifest.id
                ),
            ));
        }
        errors.extend(self.panel_origin_problems(&manifest));
        errors.extend(self.local_problems(&manifest));
        errors.extend(self.rights_problems(actor, &manifest));
        if !errors.is_empty() {
            return Ok(PreviewResult {
                errors,
                preview: None,
            });
        }
        let existing = {
            let mut conn = self.services.store().conn().await?;
            store::get(&mut conn, &manifest.id).await?
        };
        if let Some(row) = existing.as_ref().filter(|row| row.managed) {
            return Ok(PreviewResult {
                errors: vec![problem(
                    "id",
                    format!(
                        "модуль «{}» установлен из архива: обновляйте его новым архивом",
                        row.manifest.name
                    ),
                )],
                preview: None,
            });
        }
        Ok(PreviewResult {
            errors,
            preview: Some(build_preview(manifest, existing.as_ref())),
        })
    }

    async fn issue_key(
        &self,
        db: &mut impl Db,
        actor: &AdminPrincipal,
        manifest: &Manifest,
    ) -> ServiceResult<(i64, String)> {
        let token = new_token();
        let key = db
            .api_keys()
            .create(&NewApiKey {
                name: key_name(manifest),
                token_prefix: token[..SHOWN_PREFIX_LEN].to_string(),
                token_hash: token_hash(&token),
                permissions: manifest.permissions.clone(),
                created_by: actor.admin_id,
                expires_at: None,
            })
            .await?;
        Ok((key.id, token))
    }

    async fn issue_secret(
        &self,
        db: &mut impl Db,
        actor: &AdminPrincipal,
        module: &str,
    ) -> ServiceResult<String> {
        let secrets = self.services.secrets().ok_or_else(|| {
            rejected(
                "ext_modules.no_secret_key",
                "Задайте SN_SECRET_KEY: без него панель не сохранит секрет подписи модуля",
            )
        })?;
        let secret = signing::new_secret();
        write_secret(
            db,
            Some(secrets),
            &secret_key(module),
            Some(&secret),
            Some(actor.admin_id),
        )
        .await?;
        Ok(secret)
    }

    pub async fn install(
        &self,
        actor: &AdminPrincipal,
        request: &InstallRequest,
    ) -> ServiceResult<InstallResult> {
        installer(actor)?;
        let manifest = manifest::parse(&request.manifest).map_err(|problems| {
            let first = problems
                .first()
                .map(|p| {
                    if p.path.is_empty() {
                        p.message.clone()
                    } else {
                        format!("{}: {}", p.path, p.message)
                    }
                })
                .unwrap_or_default();
            rejected(
                "ext_modules.manifest",
                format!("Файл описания с ошибками. {first}"),
            )
        })?;
        if let Some(expected) = request.update.as_deref()
            && expected != manifest.id
        {
            return Err(rejected(
                "ext_modules.other_module",
                format!(
                    "В файле описан другой модуль: «{}», а обновляется «{expected}»",
                    manifest.id
                ),
            ));
        }
        if let Some(first) = self
            .panel_origin_problems(&manifest)
            .into_iter()
            .chain(self.local_problems(&manifest))
            .next()
        {
            return Err(rejected("ext_modules.manifest", first.message));
        }
        if sorted(&request.permissions) != manifest.permissions
            || sorted(&request.events) != sorted(&manifest.events)
        {
            return Err(rejected(
                "ext_modules.confirm",
                "Описание отличается от того, что вы подтвердили. Загрузите файл ещё раз и проверьте права",
            ));
        }
        if let Some(first) = self.rights_problems(actor, &manifest).into_iter().next() {
            return Err(rejected("ext_modules.permissions", first.message));
        }
        let mut tx = self.services.store().begin().await?;
        let existing = store::lock(&mut tx, &manifest.id).await?;
        let credentials = match existing {
            None => {
                let (key_id, token) = self.issue_key(&mut tx, actor, &manifest).await?;
                store::insert(&mut tx, &manifest, key_id, actor.admin_id).await?;
                let secret = self.issue_secret(&mut tx, actor, &manifest.id).await?;
                tx.audit()
                    .record(&Actor::Admin(actor.admin_id).audit(
                        "ext_module.installed",
                        "ext_module",
                        None,
                        serde_json::json!({
                            "module": manifest.id,
                            "version": manifest.version,
                            "permissions": manifest.permissions,
                            "events": manifest.events,
                            "webhook": manifest.webhook,
                            "page": page_url(&manifest),
                            "api_key": key_id,
                        }),
                    ))
                    .await?;
                Some(Credentials {
                    api_key: token,
                    signing_secret: secret,
                })
            }
            Some(row) if row.managed => {
                return Err(rejected(
                    "ext_modules.managed",
                    format!(
                        "Модуль «{}» установлен из архива: обновляйте его новым архивом",
                        row.manifest.name
                    ),
                ));
            }
            Some(row) => {
                let moved = moved_addresses(&manifest, Some(&row.manifest));
                if !moved.iter().all(|m| request.confirms(m)) {
                    return Err(rejected(
                        "ext_modules.confirm",
                        "Адрес модуля в файле отличается от того, что вы подтвердили. Загрузите файл ещё раз и проверьте адреса",
                    ));
                }
                let (config, dropped) = carry_config(&row, &manifest.settings);
                for key in dropped {
                    tx.secrets()
                        .delete(&setting_key(&manifest.id, &key))
                        .await?;
                }
                store::update(&mut tx, &manifest, &config, actor.admin_id).await?;
                if row.manifest.permissions != manifest.permissions
                    && let Some(key_id) = row.api_key_id
                {
                    tx.api_keys()
                        .set_permissions(key_id, &manifest.permissions)
                        .await?;
                }
                let added: Vec<Permission> = manifest
                    .permissions
                    .iter()
                    .filter(|p| !row.manifest.permissions.contains(p))
                    .copied()
                    .collect();
                let removed: Vec<Permission> = row
                    .manifest
                    .permissions
                    .iter()
                    .filter(|p| !manifest.permissions.contains(p))
                    .copied()
                    .collect();
                tx.audit()
                    .record(&Actor::Admin(actor.admin_id).audit(
                        "ext_module.updated",
                        "ext_module",
                        None,
                        serde_json::json!({
                            "module": manifest.id,
                            "from_version": row.manifest.version,
                            "version": manifest.version,
                            "permissions_added": added,
                            "permissions_removed": removed,
                            "events": manifest.events,
                            "webhook": manifest.webhook,
                            "moved": moved,
                        }),
                    ))
                    .await?;
                None
            }
        };
        tx.commit().await?;
        Ok(InstallResult {
            module: self.card(&manifest.id).await?,
            created: credentials.is_some(),
            credentials,
        })
    }

    async fn row(&self, id: &str) -> ServiceResult<ModuleRow> {
        let mut conn = self.services.store().conn().await?;
        store::get(&mut conn, id).await?.ok_or_else(missing)
    }

    async fn view_parts(&self, row: &ModuleRow) -> ServiceResult<(KeyInfo, Stats)> {
        let mut conn = self.services.store().conn().await?;
        let key = store::key_info(&mut conn, row.api_key_id, self.services.now()).await?;
        let stats = store::stats(&mut conn, row.id()).await?;
        Ok((key, stats))
    }

    pub async fn list(&self, actor: &AdminPrincipal) -> ServiceResult<Vec<Summary>> {
        if !actor.can(Permission::Modules) {
            return Err(ServiceError::Forbidden);
        }
        self.settle_all().await?;
        let rows = {
            let mut conn = self.services.store().conn().await?;
            store::list(&mut conn).await?
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let (key, stats) = self.view_parts(&row).await?;
            out.push(Summary {
                managed: row.managed,
                state: self.state_of(&row),
                warning: warning(&row, &key, &stats, self.loopback()),
                id: row.manifest.id.clone(),
                name: row.manifest.name.clone(),
                version: row.manifest.version.clone(),
                author: row.manifest.author.clone(),
                description: row.manifest.description.clone(),
                enabled: row.enabled,
                has_settings: !row.manifest.settings.is_empty(),
                page: page_view(&row.manifest),
            });
        }
        Ok(out)
    }

    pub async fn card(&self, id: &str) -> ServiceResult<Card> {
        let row = self.row(id).await?;
        let (key, stats) = self.view_parts(&row).await?;
        Ok(Card {
            managed: row.managed,
            takes_events: row.has_events_address(),
            warning: warning(&row, &key, &stats, self.loopback()),
            events: events_diff(&row.manifest, None),
            permissions: permissions_diff(&row.manifest, None),
            page: page_view(&row.manifest),
            client_card: client_card_view(&row.manifest),
            bot: bot_view(&row.manifest),
            inbound: inbound_view(&row.manifest),
            has_settings: !row.manifest.settings.is_empty(),
            key,
            stats,
            id: row.manifest.id,
            name: row.manifest.name,
            version: row.manifest.version,
            author: row.manifest.author,
            description: row.manifest.description,
            site: row.manifest.site,
            webhook: row.manifest.webhook,
            enabled: row.enabled,
            installed_by: row.installed_by_email,
            installed_at: row.installed_at,
            updated_at: row.updated_at,
        })
    }

    pub async fn card_for(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<Card> {
        if !actor.can(Permission::Modules) {
            return Err(ServiceError::Forbidden);
        }
        self.settle(id).await?;
        self.card(id).await
    }

    pub async fn set_enabled(
        &self,
        actor: &AdminPrincipal,
        id: &str,
        enabled: bool,
    ) -> ServiceResult<Card> {
        manager(actor)?;
        let current = self.row(id).await?;
        if current.managed {
            if current.enabled != enabled {
                self.switch_managed(actor, id, enabled).await?;
                proxy::forget(id);
            }
            return self.card(id).await;
        }
        let mut tx = self.services.store().begin().await?;
        let row = store::lock(&mut tx, id).await?.ok_or_else(missing)?;
        if row.enabled != enabled {
            store::set_enabled(&mut tx, id, enabled, actor.admin_id).await?;
            tx.audit()
                .record(&Actor::Admin(actor.admin_id).audit(
                    if enabled {
                        "ext_module.enabled"
                    } else {
                        "ext_module.disabled"
                    },
                    "ext_module",
                    None,
                    serde_json::json!({ "module": id }),
                ))
                .await?;
        }
        tx.commit().await?;
        self.card(id).await
    }

    pub async fn remove(
        &self,
        actor: &AdminPrincipal,
        id: &str,
        body: &packages::RemoveRequest,
    ) -> ServiceResult<()> {
        installer(actor)?;
        let current = self.row(id).await?;
        if current.managed {
            self.remove_managed(actor, &current, body).await?;
            proxy::forget(id);
            return Ok(());
        }
        let mut tx = self.services.store().begin().await?;
        let row = store::lock(&mut tx, id).await?.ok_or_else(missing)?;
        let revoked = match row.api_key_id {
            Some(key) => tx.api_keys().revoke(key).await?,
            None => false,
        };
        for key in tx.secrets().keys(&secrets_prefix(id)).await? {
            tx.secrets().delete(&key).await?;
        }
        store::delete(&mut tx, id).await?;
        tx.audit()
            .record(&Actor::Admin(actor.admin_id).audit(
                "ext_module.removed",
                "ext_module",
                None,
                serde_json::json!({
                    "module": id,
                    "version": row.manifest.version,
                    "api_key": row.api_key_id,
                    "api_key_revoked": revoked,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn reissue(
        &self,
        actor: &AdminPrincipal,
        id: &str,
    ) -> ServiceResult<packages::Reissued> {
        installer(actor)?;
        if self.row(id).await?.managed {
            let reissued = self.reissue_managed(actor, id).await?;
            proxy::forget(id);
            return Ok(reissued);
        }
        let mut tx = self.services.store().begin().await?;
        let row = store::lock(&mut tx, id).await?.ok_or_else(missing)?;
        if let Some(first) = self
            .rights_problems(actor, &row.manifest)
            .into_iter()
            .next()
        {
            return Err(rejected("ext_modules.permissions", first.message));
        }
        if let Some(old) = row.api_key_id {
            tx.api_keys().revoke(old).await?;
        }
        let (key_id, token) = self.issue_key(&mut tx, actor, &row.manifest).await?;
        store::set_key(&mut tx, id, key_id, actor.admin_id).await?;
        let secret = self.issue_secret(&mut tx, actor, id).await?;
        tx.audit()
            .record(&Actor::Admin(actor.admin_id).audit(
                "ext_module.credentials",
                "ext_module",
                None,
                serde_json::json!({
                    "module": id,
                    "revoked_key": row.api_key_id,
                    "api_key": key_id,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(packages::Reissued {
            credentials: Some(Credentials {
                api_key: token,
                signing_secret: secret,
            }),
            runtime: None,
        })
    }

    async fn config_view(&self, db: &mut impl Db, row: &ModuleRow) -> ServiceResult<ConfigView> {
        let mut config = Map::new();
        for field in &row.manifest.settings {
            let value = if field.is_secret() {
                let filled = db
                    .secrets()
                    .get(&setting_key(row.id(), &field.key))
                    .await?
                    .is_some();
                serde_json::json!({ "filled": filled })
            } else {
                row.config.get(&field.key).cloned().unwrap_or(Value::Null)
            };
            config.insert(field.key.clone(), value);
        }
        Ok(ConfigView {
            module: row.id().to_string(),
            schema: row.manifest.settings.clone(),
            config,
        })
    }

    pub async fn config(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<ConfigView> {
        if !actor.can(Permission::Modules) {
            return Err(ServiceError::Forbidden);
        }
        let mut conn = self.services.store().conn().await?;
        let row = store::get(&mut conn, id).await?.ok_or_else(missing)?;
        self.config_view(&mut conn, &row).await
    }

    pub async fn update_config(
        &self,
        actor: &AdminPrincipal,
        id: &str,
        update: &ConfigUpdate,
    ) -> ServiceResult<ConfigSaved> {
        manager(actor)?;
        let mut tx = self.services.store().begin().await?;
        let row = store::lock(&mut tx, id).await?.ok_or_else(missing)?;
        let fields = &row.manifest.settings;
        let find = |key: &str| {
            fields.iter().find(|f| f.key == key).ok_or_else(|| {
                rejected(
                    "ext_modules.settings",
                    format!("поля «{key}» нет в настройках модуля"),
                )
            })
        };
        let mut config = row.config.clone();
        let mut set_secrets: Vec<(String, String)> = Vec::new();
        let mut clear_secrets: Vec<String> = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        for (key, value) in &update.config {
            let field = find(key)?;
            if field.is_secret() {
                match value {
                    Value::Null => {}
                    Value::String(s) if s.trim().is_empty() => {}
                    Value::String(s) if s.trim().chars().count() <= manifest::LARGE_TEXT_MAX => {
                        set_secrets.push((field.key.clone(), s.trim().to_string()));
                        changed.push(field.key.clone());
                    }
                    Value::String(_) => {
                        return Err(value_error(field, "слишком длинное значение"));
                    }
                    _ => return Err(value_error(field, "ожидается текст")),
                }
                continue;
            }
            if blank(value) && !matches!(field.kind, FieldKind::Flag) {
                if config.remove(&field.key).is_some() {
                    changed.push(field.key.clone());
                }
                continue;
            }
            let checked = check_value(field, value)?;
            if config.get(&field.key) != Some(&checked) {
                config.insert(field.key.clone(), checked);
                changed.push(field.key.clone());
            }
        }
        for key in &update.clear {
            let field = find(key)?;
            if field.is_secret() {
                clear_secrets.push(field.key.clone());
                changed.push(field.key.clone());
            } else if config.remove(&field.key).is_some() {
                changed.push(field.key.clone());
            }
        }
        let mut lacking: Vec<String> = Vec::new();
        for field in fields.iter().filter(|f| f.required && f.default.is_none()) {
            let present = if field.is_secret() {
                set_secrets.iter().any(|(k, _)| *k == field.key)
                    || (!clear_secrets.contains(&field.key)
                        && tx
                            .secrets()
                            .get(&setting_key(id, &field.key))
                            .await?
                            .is_some())
            } else {
                config.get(&field.key).is_some_and(|v| !blank(v))
            };
            if !present {
                lacking.push(field.label.clone());
            }
        }
        if !lacking.is_empty() {
            return Err(rejected(
                "ext_modules.settings",
                format!("Заполните: {}", lacking.join(", ")),
            ));
        }
        if !set_secrets.is_empty() && self.services.secrets().is_none() {
            return Err(rejected(
                "ext_modules.no_secret_key",
                "Задайте SN_SECRET_KEY: без него панель не сохранит секретные поля",
            ));
        }
        for (key, value) in &set_secrets {
            write_secret(
                &mut tx,
                self.services.secrets(),
                &setting_key(id, key),
                Some(value),
                Some(actor.admin_id),
            )
            .await?;
        }
        for key in &clear_secrets {
            tx.secrets().delete(&setting_key(id, key)).await?;
        }
        changed.sort();
        changed.dedup();
        store::set_config(&mut tx, id, &config, actor.admin_id).await?;
        let event_queued = !changed.is_empty() && row.enabled && row.has_events_address();
        if !changed.is_empty() {
            tx.audit()
                .record(&Actor::Admin(actor.admin_id).audit(
                    "ext_module.settings",
                    "ext_module",
                    None,
                    serde_json::json!({ "module": id, "keys": changed }),
                ))
                .await?;
        }
        if event_queued {
            store::enqueue_service(
                &mut tx,
                id,
                delivery::SETTINGS_CHANGED,
                &serde_json::json!({ "keys": changed }),
                None,
            )
            .await?;
        }
        let fresh = ModuleRow { config, ..row };
        let view = self.config_view(&mut tx, &fresh).await?;
        tx.commit().await?;
        Ok(ConfigSaved { view, event_queued })
    }

    pub async fn journal(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<Journal> {
        manager(actor)?;
        let mut conn = self.services.store().conn().await?;
        if store::get(&mut conn, id).await?.is_none() {
            return Err(missing());
        }
        let rows = store::attempts(&mut conn, id, JOURNAL_MAX).await?;
        let stats = store::stats(&mut conn, id).await?;
        let mut seen: Vec<i64> = Vec::new();
        let attempts = rows
            .into_iter()
            .map(|row| {
                let latest = !seen.contains(&row.delivery_id);
                seen.push(row.delivery_id);
                AttemptView {
                    can_retry: latest
                        && matches!(row.status.as_str(), "failed" | "skipped")
                        && row.kind != delivery::PING,
                    title: kind_title(&row.kind),
                    id: row.id,
                    delivery_id: row.delivery_id,
                    kind: row.kind,
                    event_id: row.event_id,
                    attempt: row.attempt,
                    at: row.at,
                    http_status: row.http_status,
                    duration_ms: row.duration_ms,
                    error: row.error,
                    response: row.response,
                    status: row.status,
                    next_attempt_at: row.next_attempt_at,
                }
            })
            .collect();
        Ok(Journal { attempts, stats })
    }

    pub async fn retry(
        &self,
        actor: &AdminPrincipal,
        id: &str,
        delivery: i64,
    ) -> ServiceResult<bool> {
        manager(actor)?;
        let mut tx = self.services.store().begin().await?;
        let row = store::lock(&mut tx, id).await?.ok_or_else(missing)?;
        if !row.enabled {
            return Err(rejected(
                "ext_modules.disabled",
                "Модуль выключен: включите его, чтобы отправить событие снова",
            ));
        }
        let done = store::retry(&mut tx, id, delivery).await?;
        if done {
            tx.audit()
                .record(&Actor::Admin(actor.admin_id).audit(
                    "ext_module.retry",
                    "ext_module",
                    None,
                    serde_json::json!({ "module": id, "delivery": delivery }),
                ))
                .await?;
        }
        tx.commit().await?;
        Ok(done)
    }

    pub async fn retry_failed(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<u64> {
        manager(actor)?;
        let mut tx = self.services.store().begin().await?;
        let row = store::lock(&mut tx, id).await?.ok_or_else(missing)?;
        if !row.enabled {
            return Err(rejected(
                "ext_modules.disabled",
                "Модуль выключен: включите его, чтобы отправить события снова",
            ));
        }
        let queued = store::retry_failed(&mut tx, id).await?;
        if queued > 0 {
            tx.audit()
                .record(&Actor::Admin(actor.admin_id).audit(
                    "ext_module.retry",
                    "ext_module",
                    None,
                    serde_json::json!({ "module": id, "failed": queued }),
                ))
                .await?;
        }
        tx.commit().await?;
        Ok(queued)
    }

    pub async fn ping(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<PingResult> {
        manager(actor)?;
        let row = self.row(id).await?;
        if !row.enabled {
            return Err(rejected(
                "ext_modules.disabled",
                "Модуль выключен: включите его, чтобы проверить связь",
            ));
        }
        if !row.has_events_address() {
            return Err(rejected(
                "ext_modules.no_webhook",
                "У модуля нет адреса для событий: проверять нечего",
            ));
        }
        let deliverer = Deliverer::new(self.services.clone());
        let target = match deliverer.target(id).await? {
            Some((_, Ok(target))) => target,
            Some((_, Err(message))) => return Err(rejected("ext_modules.ping", message)),
            None => return Err(missing()),
        };
        let now = self.services.now();
        let claimed = {
            let mut conn = self.services.store().conn().await?;
            store::enqueue_service(
                &mut conn,
                id,
                delivery::PING,
                &serde_json::json!({}),
                Some(now + Duration::seconds(PING_LEASE_SECONDS)),
            )
            .await?
        };
        let sent = delivery::send(&target, &claimed, now).await;
        let result = PingResult {
            ok: sent.ok(),
            http_status: sent.http_status,
            duration_ms: sent.duration_ms,
            error: sent.error.clone(),
            response: sent.response.clone(),
        };
        let at = self.services.now();
        let outcome = delivery::outcome(1, sent, at, false);
        let mut conn = self.services.store().conn().await?;
        store::record(&mut conn, id, claimed.id, at, &outcome).await?;
        Ok(result)
    }

    pub async fn page_token(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<PageToken> {
        if actor.api_key.is_some() {
            return Err(ServiceError::Forbidden);
        }
        let row = self.row(id).await?;
        let page = row
            .manifest
            .page
            .clone()
            .ok_or(ServiceError::NotFound("страница модуля"))?;
        if !actor.can(page.permission) {
            return Err(ServiceError::Forbidden);
        }
        if !row.enabled {
            return Err(rejected(
                "ext_modules.disabled",
                "Модуль выключен: включите его, чтобы открыть страницу",
            ));
        }
        if !row.managed && !self.loopback() && address::local(&page.url) {
            return Err(rejected("ext_modules.page", loopback_warning()));
        }
        if !row.managed
            && self
                .services
                .settings()
                .public_url
                .as_deref()
                .is_some_and(|panel| same_origin(&page.url, panel))
        {
            return Err(rejected(
                "ext_modules.page",
                "Страница модуля лежит на адресе самой панели: так открывать её небезопасно",
            ));
        }
        let mut conn = self.services.store().conn().await?;
        let key = read_secret(&mut conn, self.services.secrets(), &secret_key(id))
            .await?
            .as_deref()
            .and_then(signing::key)
            .ok_or_else(|| {
                rejected(
                    "ext_modules.secret",
                    "Секрет подписи модуля не прочитан: выпустите новый ключ и секрет",
                )
            })?;
        let admin = conn.admins().get(actor.admin_id).await?;
        let now = self.services.now();
        let (claims, expires_at) = admin_claims(actor, admin.email, id, now);
        let token = signing::page_token(&key, &claims)
            .ok_or_else(|| ServiceError::Internal("токен страницы не подписан".into()))?;
        if row.managed {
            let cap = proxy::cap(
                &key,
                id,
                actor.admin_id.0,
                actor.session_id,
                (now + Duration::hours(proxy::CAP_HOURS)).timestamp(),
            );
            let path = page.path.as_deref().unwrap_or("/");
            let mut url = reqwest::Url::parse("http://panel.invalid")
                .and_then(|base| base.join(&format!("{}{id}/{cap}{path}", proxy::PREFIX)))
                .map_err(|_| rejected("ext_modules.page", "Адрес страницы модуля не разобран"))?;
            url.query_pairs_mut()
                .append_pair(signing::TOKEN_PARAM, &token);
            let relative = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_string(),
            };
            return Ok(PageToken {
                origin: "null".into(),
                url: relative,
                token,
                expires_at,
                proxied: true,
                title: page.title,
                module: row.manifest.name,
            });
        }
        let mut url = reqwest::Url::parse(&page.url)
            .map_err(|_| rejected("ext_modules.page", "Адрес страницы модуля не разобран"))?;
        url.query_pairs_mut()
            .append_pair(signing::TOKEN_PARAM, &token);
        Ok(PageToken {
            origin: url.origin().ascii_serialization(),
            url: url.to_string(),
            token,
            expires_at,
            proxied: false,
            title: page.title,
            module: row.manifest.name,
        })
    }

    pub async fn menu(&self, actor: &AdminPrincipal) -> ServiceResult<Vec<MenuPage>> {
        if actor.api_key.is_some() {
            return Ok(Vec::new());
        }
        let rows = {
            let mut conn = self.services.store().conn().await?;
            store::list(&mut conn).await?
        };
        let mut pages: Vec<MenuPage> = rows
            .into_iter()
            .filter(|row| row.enabled)
            .filter_map(|row| {
                let page = row.manifest.page?;
                actor.can(page.permission).then_some(MenuPage {
                    id: row.manifest.id,
                    title: page.title,
                })
            })
            .collect();
        pages.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
        Ok(pages)
    }

    pub async fn me(&self, principal: &AdminPrincipal) -> ServiceResult<SelfView> {
        let key_id = principal.api_key.ok_or_else(|| {
            rejected(
                "ext_modules.not_module",
                "Этот адрес — для ключа внешнего модуля",
            )
        })?;
        let mut conn = self.services.store().conn().await?;
        let row = store::by_key(&mut conn, key_id).await?.ok_or_else(|| {
            rejected(
                "ext_modules.not_module",
                "Этот ключ не принадлежит внешнему модулю",
            )
        })?;
        let mut settings = Map::new();
        for field in &row.manifest.settings {
            let value = if field.is_secret() {
                read_secret(
                    &mut conn,
                    self.services.secrets(),
                    &setting_key(row.id(), &field.key),
                )
                .await?
                .map(Value::String)
                .unwrap_or(Value::Null)
            } else {
                row.config
                    .get(&field.key)
                    .cloned()
                    .unwrap_or_else(|| typed_default(field))
            };
            settings.insert(field.key.clone(), value);
        }
        Ok(SelfView {
            id: row.manifest.id,
            name: row.manifest.name,
            version: row.manifest.version,
            enabled: row.enabled,
            events: row.manifest.events,
            permissions: principal.permissions.clone(),
            settings,
        })
    }
}
