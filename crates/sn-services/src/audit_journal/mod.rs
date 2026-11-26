pub mod labels;
pub mod scrub;

#[cfg(test)]
mod scan_tests;
#[cfg(test)]
mod tests;

use crate::error::{ServiceError, ServiceResult};
use crate::ext_modules::manifest::permission_title;
use crate::services::Services;
use chrono::{DateTime, TimeZone, Utc};
use labels::Area;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::access::Permission;
use sn_domain::format;
use sn_domain::module::ModuleId;
use sn_domain::money::{Currency, Money};
use sn_store::{
    AuditActorFilter, AuditCursor, AuditFilter, AuditRefs, AuditRow, AuditSearch, Db, like_pattern,
};
use std::collections::HashMap;
use std::net::IpAddr;

pub const PAGE_SIZE: i64 = 50;
pub const PAGE_MAX: i64 = 100;
pub const SEARCH_MIN: usize = 3;
const SEARCH_MAX: usize = 200;
const TEXT_MAX: usize = 600;
const SCAN_ROUNDS: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct JournalQuery {
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub client: Option<i64>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActorView {
    Admin {
        id: i64,
        email: Option<String>,
        role: Option<String>,
    },
    ApiKey {
        id: i64,
        name: Option<String>,
        owner: Option<String>,
    },
    Module {
        id: String,
        name: Option<String>,
    },
    Client {
        id: i64,
        name: String,
    },
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetView {
    pub kind: String,
    pub label: String,
    pub note: Option<String>,
    pub link: Option<String>,
    pub client_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangeView {
    pub label: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldView {
    pub key: String,
    pub label: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalItem {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub action: String,
    pub title: String,
    pub area: Option<Area>,
    pub known: bool,
    pub actor: ActorView,
    pub via_unknown: bool,
    pub target: Option<TargetView>,
    pub changes: Vec<ChangeView>,
    pub fields: Vec<FieldView>,
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalPage {
    pub items: Vec<JournalItem>,
    pub next: Option<String>,
    pub client: Option<TargetView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientChoice {
    pub id: i64,
    pub label: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetaAdmin {
    pub id: i64,
    pub email: String,
    pub role: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetaKey {
    pub id: i64,
    pub name: String,
    pub owner: Option<String>,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetaModule {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetaAction {
    pub code: &'static str,
    pub title: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetaArea {
    pub id: Area,
    pub title: &'static str,
    pub actions: Vec<MetaAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalMeta {
    pub admins: Vec<MetaAdmin>,
    pub keys: Vec<MetaKey>,
    pub modules: Vec<MetaModule>,
    pub areas: Vec<MetaArea>,
    pub search_min: usize,
}

fn rejected(code: &'static str, message: &str) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn actor_filter(raw: &str) -> ServiceResult<AuditActorFilter> {
    let unknown = || rejected("audit.actor", "Неизвестный участник: обновите страницу");
    let number = |text: &str| {
        text.parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(unknown)
    };
    match raw.split_once(':') {
        None if raw == "system" => Ok(AuditActorFilter::System),
        None if raw == "clients" => Ok(AuditActorFilter::Clients),
        Some(("admin", id)) => Ok(AuditActorFilter::Admin(number(id)?)),
        Some(("key", id)) => Ok(AuditActorFilter::ApiKey(number(id)?)),
        Some(("client", id)) => Ok(AuditActorFilter::Client(number(id)?)),
        Some(("module", id))
            if !id.is_empty()
                && id.len() <= 40
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') =>
        {
            Ok(AuditActorFilter::Module(id.to_string()))
        }
        _ => Err(unknown()),
    }
}

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 80
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

pub fn filter_of(query: &JournalQuery) -> ServiceResult<AuditFilter> {
    let actor = query
        .actor
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(actor_filter)
        .transpose()?;
    let actions = match (
        query.action.as_deref().map(str::trim),
        query.area.as_deref().map(str::trim),
    ) {
        (Some(code), _) if !code.is_empty() => {
            if !valid_code(code) {
                return Err(rejected(
                    "audit.action",
                    "Неизвестное действие: обновите страницу",
                ));
            }
            Some(vec![code.to_string()])
        }
        (_, Some(area)) if !area.is_empty() => {
            let area = Area::parse(area)
                .ok_or_else(|| rejected("audit.area", "Неизвестный раздел: обновите страницу"))?;
            Some(labels::codes_in(area))
        }
        _ => None,
    };
    if let (Some(since), Some(until)) = (query.since, query.until)
        && since >= until
    {
        return Err(rejected(
            "audit.period",
            "Начало периода должно быть раньше конца",
        ));
    }
    let search = match query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        None => None,
        Some(q) if q.chars().count() < SEARCH_MIN => {
            return Err(rejected(
                "audit.search_short",
                "Для поиска нужно хотя бы 3 символа",
            ));
        }
        Some(q) => {
            let q: String = q.chars().take(SEARCH_MAX).collect();
            Some(match q.parse::<IpAddr>() {
                Ok(ip) => AuditSearch::Ip(ip.to_string()),
                Err(_) => AuditSearch::Text {
                    pattern: like_pattern(&q),
                    actions: labels::codes_matching(&q),
                },
            })
        }
    };
    Ok(AuditFilter {
        actor,
        actions,
        client: query.client.filter(|id| *id > 0),
        since: query.since,
        until: query.until,
        search,
    })
}

pub fn cursor_of(row: &AuditRow) -> String {
    format!("{}.{}", row.created_at.timestamp_micros(), row.id)
}

pub fn parse_cursor(raw: &str) -> ServiceResult<AuditCursor> {
    let bad = || rejected("audit.cursor", "Список устарел: обновите страницу");
    let (micros, id) = raw.split_once('.').ok_or_else(bad)?;
    let micros: i64 = micros.parse().map_err(|_| bad())?;
    let id: i64 = id.parse().map_err(|_| bad())?;
    let at = Utc.timestamp_micros(micros).single().ok_or_else(bad)?;
    Ok(AuditCursor { at, id })
}

fn clip(text: &str) -> String {
    if text.chars().count() > TEXT_MAX {
        let mut out: String = text.chars().take(TEXT_MAX - 1).collect();
        out.push('…');
        out
    } else {
        text.to_string()
    }
}

fn role_title(role: &str) -> Option<&'static str> {
    match role {
        "owner" => Some("Владелец"),
        "admin" => Some("Администратор"),
        "support" => Some("Поддержка"),
        "readonly" | "read_only" => Some("Только просмотр"),
        _ => None,
    }
}

fn money_of(map: &Map<String, Value>) -> Option<String> {
    if map.len() != 2 {
        return None;
    }
    let minor = map.get("minor")?.as_i64()?;
    let currency = map.get("currency")?.as_str()?;
    if currency.eq_ignore_ascii_case("XTR") {
        return Some(format!(
            "{} {}",
            format::group_digits(minor.unsigned_abs()),
            format::plural_ru(minor.unsigned_abs(), "звезда", "звезды", "звёзд")
        ));
    }
    Currency::parse(currency)
        .ok()
        .map(|currency| format::money(Money::new(minor, currency)))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub providers: HashMap<String, String>,
    pub methods: HashMap<String, String>,
}

impl Names {
    fn of(services: &Services) -> Names {
        let mut names = Names::default();
        for provider in sn_modules::CoreApi::payment_titles(services) {
            for method in provider.methods {
                names.methods.entry(method.code).or_insert(method.title);
            }
            names.providers.insert(provider.provider, provider.title);
        }
        names
    }

    fn text(&self, key: &str, value: &str) -> Option<String> {
        match key {
            "provider" => self.providers.get(value).cloned(),
            "method" | "methods" => self.methods.get(value).cloned(),
            _ => labels::value_label(key, value).map(str::to_string),
        }
    }
}

fn text_of(names: &Names, key: &str, value: &Value) -> String {
    match value {
        Value::Null => "—".into(),
        Value::Bool(true) => "да".into(),
        Value::Bool(false) => "нет".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) if key == "role" || key.ends_with("_role") => {
            role_title(s).map_or_else(|| clip(s), str::to_string)
        }
        Value::String(s) if key == "permission" => Permission::parse(s)
            .map(|p| permission_title(p).to_string())
            .unwrap_or_else(|_| clip(s)),
        Value::String(s) if s.is_empty() => "—".into(),
        Value::String(s) => names.text(key, s).unwrap_or_else(|| clip(s)),
        Value::Array(items) if items.is_empty() => "—".into(),
        Value::Array(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|item| match (key, item) {
                    ("permissions", Value::String(s)) => Permission::parse(s)
                        .map(|p| permission_title(p).to_string())
                        .unwrap_or_else(|_| s.clone()),
                    (_, Value::String(s)) => names.text(key, s).unwrap_or_else(|| s.clone()),
                    _ => text_of(names, "", item),
                })
                .collect();
            clip(&parts.join(", "))
        }
        Value::Object(map) => {
            money_of(map).unwrap_or_else(|| clip(&Value::Object(map.clone()).to_string()))
        }
    }
}

fn label_of(action: &str, key: &str) -> String {
    labels::field_label_in(action, key).map_or_else(|| key.to_string(), str::to_string)
}

fn changes_of(
    names: &Names,
    action: &str,
    details: &Value,
) -> (Vec<ChangeView>, Map<String, Value>) {
    let mut rest = details.as_object().cloned().unwrap_or_default();
    let mut changes = Vec::new();
    match (rest.get("from").cloned(), rest.get("to").cloned()) {
        (Some(Value::Object(from)), Some(Value::Object(to))) => {
            let mut keys: Vec<&String> = from.keys().chain(to.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let before = from.get(key).unwrap_or(&Value::Null);
                let after = to.get(key).unwrap_or(&Value::Null);
                if before != after {
                    changes.push(ChangeView {
                        label: label_of(action, key),
                        before: text_of(names, key, before),
                        after: text_of(names, key, after),
                    });
                }
            }
            rest.remove("from");
            rest.remove("to");
        }
        (Some(from), Some(to)) => {
            let key = if action == "admin.role_changed" {
                "role"
            } else {
                ""
            };
            changes.push(ChangeView {
                label: if key.is_empty() {
                    "Значение".into()
                } else {
                    "Роль".into()
                },
                before: text_of(names, key, &from),
                after: text_of(names, key, &to),
            });
            rest.remove("from");
            rest.remove("to");
        }
        _ => {}
    }
    if let (Some(previous), Some(email)) =
        (rest.get("previous").cloned(), rest.get("email").cloned())
    {
        changes.push(ChangeView {
            label: "Почта".into(),
            before: text_of(names, "", &previous),
            after: text_of(names, "", &email),
        });
        rest.remove("previous");
        rest.remove("email");
    }
    let paired: Vec<String> = rest
        .keys()
        .filter_map(|key| key.strip_suffix("_before"))
        .filter(|base| rest.contains_key(*base))
        .map(str::to_string)
        .collect();
    for base in paired {
        let before = rest
            .remove(&format!("{base}_before"))
            .unwrap_or(Value::Null);
        let after = rest.remove(&base).unwrap_or(Value::Null);
        changes.push(ChangeView {
            label: label_of(action, &base),
            before: text_of(names, &base, &before),
            after: text_of(names, &base, &after),
        });
    }
    if action == "admin.permission_changed"
        && let (Some(Value::String(permission)), Some(Value::Bool(on))) =
            (rest.get("permission").cloned(), rest.get("on").cloned())
    {
        changes.push(ChangeView {
            label: text_of(names, "permission", &Value::String(permission)),
            before: if on {
                "нет".into()
            } else {
                "есть".into()
            },
            after: if on {
                "есть".into()
            } else {
                "нет".into()
            },
        });
        rest.remove("permission");
        rest.remove("on");
    }
    (changes, rest)
}

fn empty(key: &str, value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        Value::String(text) => text.trim().is_empty(),
        Value::Bool(_) => key == "changed" || key == "cleared",
        Value::Number(_) => false,
    }
}

fn fields_of(names: &Names, action: &str, rest: Map<String, Value>) -> Vec<FieldView> {
    rest.into_iter()
        .filter(|(key, value)| !empty(key, value))
        .map(|(key, value)| FieldView {
            label: labels::field_label_in(action, &key).map(str::to_string),
            value: text_of(names, &key, &value),
            key,
        })
        .collect()
}

pub fn title_for(action: &str, details: &Value) -> String {
    let flag = |key: &str| details.get(key).and_then(Value::as_bool);
    match action {
        "modules.toggle" => match flag("enabled") {
            Some(true) => "Модуль включён".into(),
            Some(false) => "Модуль выключен".into(),
            None => labels::title_of(action).into(),
        },
        "client.note" if flag("cleared") == Some(true) => "Удалена заметка о клиенте".into(),
        "tariff.on_sale" => match flag("on_sale") {
            Some(true) => "Тариф выставлен на продажу".into(),
            Some(false) => "Тариф снят с продажи".into(),
            None => labels::title_of(action).into(),
        },
        "admin.permission_changed" => {
            let permission = details
                .get("permission")
                .and_then(Value::as_str)
                .and_then(|p| Permission::parse(p).ok())
                .map(permission_title);
            match (permission, flag("on")) {
                (Some(title), Some(true)) => format!("Выдано право «{title}»"),
                (Some(title), Some(false)) => format!("Снято право «{title}»"),
                _ => labels::title_of(action).into(),
            }
        }
        "ticket.bulk_closed" => match details.get("count").and_then(Value::as_u64) {
            Some(count) => format!("Закрыто обращений в поддержку: {count}"),
            None => labels::title_of(action).into(),
        },
        "subscription.add_days" | "subscription.add_days_undone" => {
            match details
                .get("days")
                .and_then(Value::as_u64)
                .and_then(|d| u32::try_from(d).ok())
            {
                Some(days) if action == "subscription.add_days" => {
                    format!("Добавлено к подписке: {}", format::days(days))
                }
                Some(days) => format!("Отменено добавление: {}", format::days(days)),
                None => labels::title_of(action).into(),
            }
        }
        _ => labels::title_of(action).into(),
    }
}

fn person(refs: &AuditRefs, id: i64) -> String {
    refs.clients
        .get(&id)
        .and_then(|p| {
            p.name
                .clone()
                .or_else(|| p.telegram_username.as_ref().map(|u| format!("@{u}")))
                .or_else(|| p.email.clone())
        })
        .unwrap_or_else(|| format!("Клиент №{id}"))
}

fn module_name(refs: &AuditRefs, id: &str, key_name: Option<&str>) -> Option<String> {
    refs.modules.get(id).cloned().or_else(|| {
        key_name.map(|name| {
            name.strip_prefix("Модуль «")
                .and_then(|n| n.strip_suffix('»'))
                .unwrap_or(name)
                .to_string()
        })
    })
}

pub fn actor_of(row: &AuditRow, refs: &AuditRefs) -> ActorView {
    if let Some(key) = row.api_key_id {
        let info = refs.keys.get(&key);
        let module = row
            .ext_module
            .clone()
            .or_else(|| info.and_then(|k| k.module.clone()));
        if let Some(id) = module {
            let name = module_name(refs, &id, info.map(|k| k.name.as_str()));
            return ActorView::Module { id, name };
        }
        return ActorView::ApiKey {
            id: key,
            name: info.map(|k| k.name.clone()),
            owner: info.and_then(|k| k.owner.clone()),
        };
    }
    match (row.actor_kind.as_str(), row.actor_id) {
        ("admin", Some(id)) => {
            let admin = refs.admins.get(&id);
            ActorView::Admin {
                id,
                email: admin.map(|a| a.email.clone()),
                role: admin.and_then(|a| role_title(&a.role)).map(str::to_string),
            }
        }
        ("client", Some(id)) => ActorView::Client {
            id,
            name: person(refs, id),
        },
        _ => ActorView::System,
    }
}

fn place(kind: &str, label: &str, link: &str) -> Option<TargetView> {
    Some(TargetView {
        kind: kind.into(),
        label: label.into(),
        note: None,
        link: Some(link.into()),
        client_id: None,
    })
}

fn settings_place(action: &str) -> Option<TargetView> {
    let (label, link) = match action {
        "settings.core" | "settings.currency" => ("Общие настройки", "/admin/settings/general"),
        "settings.proxy"
        | "settings.proxy_reset"
        | "settings.proxy_down"
        | "settings.proxy_restored" => ("Исходящий прокси", "/admin/settings/general#proxy"),
        "settings.appearance" | "settings.site" | "settings.logo" | "settings.favicon" => {
            ("Оформление", "/admin/settings/look")
        }
        "settings.trial" | "settings.multi_subscriptions" => ("Тарифы", "/admin/tariffs"),
        "settings.client_access" | "settings.bot_menu" => {
            ("Доступ клиентов", "/admin/settings/client")
        }
        "settings.languages" => ("Языки", "/admin/settings/languages"),
        "settings.autorenew"
        | "settings.payment_order"
        | "settings.provider"
        | "settings.provider_check" => ("Платёжки", "/admin/settings/payments"),
        "settings.bot"
        | "settings.bot_takeover"
        | "settings.bot_updates"
        | "settings.bot_webhook_broken"
        | "settings.bot_webhook_restored"
        | "settings.bot_webhook_fallback"
        | "settings.bot_flood"
        | "settings.bot_flood_blocked"
        | "settings.bot_signups_limited" => ("Бот", "/admin/settings/bot"),
        "settings.bot_content" => ("Тексты бота", "/admin/settings/bot/texts"),
        "settings.bot_image" => ("Картинки бота", "/admin/settings/bot/images"),
        "settings.documents" => ("Документы", "/admin/settings/documents"),
        "settings.landing" | "settings.landing_image" => ("Лендинг", "/admin/settings/landing"),
        "settings.cabinet" | "settings.cabinet_image" => {
            ("Кабинет на сайте", "/admin/settings/cabinet")
        }
        "settings.miniapp" | "settings.miniapp_image" => {
            ("Мини-приложение", "/admin/settings/miniapp")
        }
        "settings.connect"
        | "settings.connect_apps"
        | "settings.connect_app_icon"
        | "settings.connect_apps_import"
        | "settings.remnawave"
        | "settings.remnawave_subscription"
        | "settings.remnawave_webhook_secret" => ("Remnawave", "/admin/settings/remnawave"),
        "mail.letters" => ("Письма", "/admin/settings/mail/letters"),
        _ => return None,
    };
    place("settings", label, link)
}

fn client_target(kind: &str, refs: &AuditRefs, client: i64, note: Option<String>) -> TargetView {
    TargetView {
        kind: kind.into(),
        label: person(refs, client),
        note,
        link: Some(format!("/admin/clients/{client}")),
        client_id: Some(client),
    }
}

fn detail_text<'v>(details: &'v Value, key: &str) -> Option<&'v str> {
    details
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn target_of(
    row: &AuditRow,
    details: &Value,
    refs: &AuditRefs,
    names: &Names,
) -> Option<TargetView> {
    let action = row.action.as_str();
    let id = row.entity_id;
    match (row.entity_kind.as_deref(), id) {
        (Some("client"), Some(id)) => Some(client_target("client", refs, id, None)),
        (Some("subscription"), Some(id)) => match refs.subscriptions.get(&id) {
            Some(sub) => {
                let note = match &sub.tariff {
                    Some(tariff) => format!("подписка №{id}, тариф «{tariff}»"),
                    None => format!("подписка №{id}"),
                };
                Some(client_target(
                    "subscription",
                    refs,
                    sub.client_id,
                    Some(note),
                ))
            }
            None => Some(TargetView {
                kind: "subscription".into(),
                label: format!("Подписка №{id}"),
                note: None,
                link: None,
                client_id: None,
            }),
        },
        (Some("order"), Some(id)) => match refs.orders.get(&id) {
            Some(client) => Some(client_target(
                "order",
                refs,
                *client,
                Some(format!("заказ №{id}")),
            )),
            None => Some(TargetView {
                kind: "order".into(),
                label: format!("Заказ №{id}"),
                note: None,
                link: None,
                client_id: None,
            }),
        },
        (Some("payment"), Some(id)) => {
            let payment = refs.payments.get(&id);
            let amount = payment.and_then(|p| {
                let mut map = Map::new();
                map.insert("minor".into(), Value::from(p.amount_minor));
                map.insert("currency".into(), Value::from(p.currency.clone()));
                money_of(&map)
            });
            Some(TargetView {
                kind: "payment".into(),
                label: match amount {
                    Some(amount) => format!("Платёж №{id} · {amount}"),
                    None => format!("Платёж №{id}"),
                },
                note: payment.map(|p| person(refs, p.client_id)),
                link: Some(format!("/admin/money?payment={id}")),
                client_id: payment.map(|p| p.client_id),
            })
        }
        (Some("ticket"), Some(id)) => {
            let ticket = refs.tickets.get(&id);
            Some(TargetView {
                kind: "ticket".into(),
                label: format!("Обращение №{id}"),
                note: ticket.map(|t| match &t.subject {
                    Some(subject) if !subject.trim().is_empty() => {
                        format!("{} · «{}»", person(refs, t.client_id), clip(subject.trim()))
                    }
                    _ => person(refs, t.client_id),
                }),
                link: Some(format!("/admin/support?ticket={id}")),
                client_id: ticket.map(|t| t.client_id),
            })
        }
        (Some("tariff"), _) => {
            let name = id
                .and_then(|id| refs.tariffs.get(&id).cloned())
                .or_else(|| detail_text(details, "name").map(str::to_string));
            place(
                "tariff",
                &name.map_or_else(|| "Тарифы".to_string(), |n| format!("Тариф «{n}»")),
                "/admin/tariffs",
            )
        }
        (Some("category"), _) => {
            let name = id
                .and_then(|id| refs.categories.get(&id).cloned())
                .or_else(|| detail_text(details, "name").map(str::to_string));
            place(
                "category",
                &name.map_or_else(|| "Тарифы".to_string(), |n| format!("Категория «{n}»")),
                "/admin/tariffs",
            )
        }
        (Some("admin"), _) => {
            let email = id
                .and_then(|id| refs.admins.get(&id))
                .map(|a| a.email.clone());
            place(
                "admin",
                &email.unwrap_or_else(|| "Администраторы".into()),
                "/admin/settings/admins",
            )
        }
        (Some("api_key"), _) => {
            let name = id.and_then(|id| refs.keys.get(&id)).map(|k| k.name.clone());
            place(
                "api_key",
                &name.map_or_else(|| "Ключи API".to_string(), |n| format!("Ключ «{n}»")),
                "/admin/settings/api-keys",
            )
        }
        (Some("ext_module" | "domain_event"), _) => match detail_text(details, "module") {
            Some(module) => {
                let name = module_name(refs, module, None).unwrap_or_else(|| module.to_string());
                place(
                    "ext_module",
                    &format!("Модуль «{name}»"),
                    &format!("/admin/modules/ext/{module}"),
                )
            }
            None => place("ext_module", "Модули", "/admin/modules"),
        },
        (Some("module"), _) => match detail_text(details, "module") {
            Some(module) => {
                let title = ModuleId::parse(module).map_or(module, |m| m.info().title);
                place(
                    "module",
                    &format!("Модуль «{title}»"),
                    &format!("/admin/modules/{module}"),
                )
            }
            None => place("module", "Модули", "/admin/modules"),
        },
        (Some("backup"), _) => place("backup", "Резервные копии", "/admin/settings/backups"),
        (Some("system"), _) => place("system", "Обновления", "/admin/settings/updates"),
        (Some("import"), _) => place("import", "Перенос", "/admin/settings/import"),
        (Some("remna_transfer"), _) => place(
            "remna_transfer",
            "Синхронизация с Remnawave",
            "/admin/settings/remnawave/sync",
        ),
        (Some("node"), _) => place("node", "Ноды", "/admin/infra"),
        (Some("host"), _) => place("host", "Хосты", "/admin/infra/hosts"),
        (Some("squad"), _) => place("squad", "Сквады", "/admin/infra/squads"),
        (Some("provider"), _) if action.starts_with("provider.") => {
            place("provider", "Оплата серверов", "/admin/infra/billing")
        }
        (Some("broadcast"), Some(id))
            if action == "broadcast.unsubscribed" || action == "broadcast.resubscribed" =>
        {
            match row.actor_id.filter(|_| row.actor_kind == "client") {
                Some(client) => Some(client_target(
                    "client",
                    refs,
                    client,
                    Some(format!("рассылка №{id}")),
                )),
                None => place(
                    "broadcast",
                    &format!("Рассылка №{id}"),
                    &format!("/admin/m/broadcast/{id}"),
                ),
            }
        }
        (Some("broadcast"), Some(id)) => place(
            "broadcast",
            &format!("Рассылка №{id}"),
            &format!("/admin/m/broadcast/{id}"),
        ),
        (Some("broadcast_rule"), Some(id)) => place(
            "broadcast_rule",
            &format!("Авторассылка №{id}"),
            &format!("/admin/m/broadcast/auto/{id}"),
        ),
        (Some("contest"), Some(id)) => place(
            "contest",
            &format!("Конкурс №{id}"),
            &format!("/admin/m/contests/{id}"),
        ),
        (Some("roulette"), _) => place("roulette", "Рулетка", "/admin/m/roulette"),
        (Some("payment_notice"), Some(id)) => place(
            "payment_notice",
            &format!("Уведомление №{id}"),
            "/admin/money",
        ),
        _ => {
            if action == "settings.provider" || action == "settings.provider_check" {
                let provider = detail_text(details, "provider");
                let label = provider.map(|id| names.providers.get(id).map_or(id, String::as_str));
                return place(
                    "provider",
                    &label.map_or_else(
                        || "Платёжки".to_string(),
                        |name| format!("Платёжка «{name}»"),
                    ),
                    "/admin/settings/payments",
                );
            }
            if let Some(found) = settings_place(action) {
                return Some(found);
            }
            if action.starts_with("promolinks.") {
                let label = detail_text(details, "code").map_or_else(
                    || "Промо-ссылки".to_string(),
                    |code| format!("Промо-ссылка «{code}»"),
                );
                return place("promolink", &label, "/admin/m/promolinks");
            }
            if action.starts_with("gifts.") {
                return place("gifts", "Подарки", "/admin/m/gifts");
            }
            if action.starts_with("backups.") {
                return place("backup", "Резервные копии", "/admin/settings/backups");
            }
            if action.starts_with("system.") {
                return place("system", "Обновления", "/admin/settings/updates");
            }
            None
        }
    }
}

fn fold(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

pub fn visible_text(item: &JournalItem) -> String {
    let mut parts: Vec<&str> = vec![&item.title, &item.action];
    match &item.actor {
        ActorView::Admin { email, role, .. } => {
            parts.extend(email.as_deref());
            parts.extend(role.as_deref());
        }
        ActorView::ApiKey { name, owner, .. } => {
            parts.extend(name.as_deref());
            parts.extend(owner.as_deref());
        }
        ActorView::Module { id, name } => {
            parts.push(id);
            parts.extend(name.as_deref());
        }
        ActorView::Client { name, .. } => parts.push(name),
        ActorView::System => {}
    }
    if let Some(target) = &item.target {
        parts.push(&target.label);
        parts.extend(target.note.as_deref());
    }
    for change in &item.changes {
        parts.extend([change.label.as_str(), &change.before, &change.after]);
    }
    for field in &item.fields {
        parts.extend([field.key.as_str(), &field.value]);
        parts.extend(field.label.as_deref());
    }
    parts.extend(item.ip.as_deref());
    fold(&parts.join("\n"))
}

pub fn item_of(row: &AuditRow, refs: &AuditRefs, names: &Names) -> JournalItem {
    let details = scrub::scrub(&row.details);
    let (changes, rest) = changes_of(names, &row.action, &details);
    JournalItem {
        id: row.id,
        at: row.created_at,
        action: row.action.clone(),
        title: title_for(&row.action, &details),
        area: labels::area_of(&row.action),
        known: labels::label(&row.action).is_some(),
        actor: actor_of(row, refs),
        via_unknown: row.actor_kind == "admin" && row.api_key_id.is_none() && !row.via_recorded,
        target: target_of(row, &details, refs, names),
        changes,
        fields: fields_of(names, &row.action, rest),
        ip: row.ip.clone(),
    }
}

pub struct AuditJournalOps<'a> {
    services: &'a Services,
}

impl<'a> AuditJournalOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        AuditJournalOps { services }
    }

    pub async fn page(&self, query: &JournalQuery) -> ServiceResult<JournalPage> {
        let filter = filter_of(query)?;
        let mut from = query.cursor.as_deref().map(parse_cursor).transpose()?;
        let limit = query.limit.unwrap_or(PAGE_SIZE).clamp(1, PAGE_MAX);
        let needle = match &filter.search {
            Some(AuditSearch::Text { .. }) => query.q.as_deref().map(|q| fold(q.trim())),
            _ => None,
        };
        let mut conn = self.services.store().conn().await?;
        let client = match filter.client {
            Some(id) => {
                let refs = AuditRefs {
                    clients: conn
                        .clients()
                        .people(&[sn_domain::ids::ClientId(id)])
                        .await?
                        .into_iter()
                        .map(|(id, person)| (id.0, person))
                        .collect(),
                    ..AuditRefs::default()
                };
                Some(client_target("client", &refs, id, None))
            }
            None => None,
        };
        let names = Names::of(self.services);
        let mut items = Vec::new();
        for round in 0..SCAN_ROUNDS {
            let mut rows = conn.audit_journal().page(&filter, from, limit + 1).await?;
            let exhausted = rows.len() <= limit as usize;
            rows.truncate(limit as usize);
            let refs = conn.audit_journal().refs(&rows).await?;
            let mut stopped_at = None;
            for (index, row) in rows.iter().enumerate() {
                from = Some(AuditCursor {
                    at: row.created_at,
                    id: row.id,
                });
                let item = item_of(row, &refs, &names);
                if needle
                    .as_deref()
                    .is_none_or(|q| visible_text(&item).contains(q))
                {
                    items.push(item);
                    if items.len() == limit as usize {
                        stopped_at = Some(index);
                        break;
                    }
                }
            }
            let last = rows.len().saturating_sub(1);
            let done = match stopped_at {
                Some(index) => exhausted && index == last,
                None => exhausted,
            };
            if done {
                return Ok(JournalPage {
                    items,
                    next: None,
                    client,
                });
            }
            if stopped_at.is_some() || round + 1 == SCAN_ROUNDS {
                break;
            }
        }
        Ok(JournalPage {
            items,
            next: from.map(|c| format!("{}.{}", c.at.timestamp_micros(), c.id)),
            client,
        })
    }

    pub async fn clients(&self, query: &str) -> ServiceResult<Vec<ClientChoice>> {
        let query: String = query.trim().chars().take(SEARCH_MAX).collect();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let page = self
            .services
            .clients()
            .search(
                &sn_store::ClientFilter {
                    query: Some(query),
                    ..sn_store::ClientFilter::default()
                },
                sn_domain::paging::PageRequest::new(Some(8), None),
            )
            .await?;
        Ok(page
            .items
            .into_iter()
            .map(|item| {
                let id = item.client.id.0;
                let username = item.telegram_username.map(|u| format!("@{u}"));
                let label = item
                    .name
                    .clone()
                    .or_else(|| username.clone())
                    .or_else(|| item.email.clone())
                    .unwrap_or_else(|| format!("Клиент №{id}"));
                let note = [username, item.email, Some(format!("№{id}"))]
                    .into_iter()
                    .flatten()
                    .filter(|part| *part != label)
                    .collect::<Vec<_>>()
                    .join(" · ");
                ClientChoice {
                    id,
                    label,
                    note: (!note.is_empty()).then_some(note),
                }
            })
            .collect())
    }

    pub async fn entry(&self, id: i64) -> ServiceResult<JournalItem> {
        let mut conn = self.services.store().conn().await?;
        let row = conn
            .audit_journal()
            .entry(id)
            .await?
            .ok_or(ServiceError::NotFound("запись журнала"))?;
        let refs = conn
            .audit_journal()
            .refs(std::slice::from_ref(&row))
            .await?;
        Ok(item_of(&row, &refs, &Names::of(self.services)))
    }

    pub async fn meta(&self) -> ServiceResult<JournalMeta> {
        let mut conn = self.services.store().conn().await?;
        let admins = conn.audit_journal().admins().await?;
        let keys = conn.audit_journal().keys().await?;
        let modules = conn.audit_journal().modules().await?;
        let key_names: Vec<(String, String)> = keys
            .iter()
            .filter_map(|k| k.module.clone().map(|m| (m, k.name.clone())))
            .collect();
        Ok(JournalMeta {
            admins: admins
                .into_iter()
                .map(|a| MetaAdmin {
                    role: role_title(&a.role).unwrap_or("Администратор").to_string(),
                    id: a.id,
                    email: a.email,
                    disabled: a.disabled,
                })
                .collect(),
            keys: keys
                .into_iter()
                .filter(|k| k.module.is_none())
                .map(|k| MetaKey {
                    id: k.id,
                    name: k.name,
                    owner: k.owner,
                    revoked: k.revoked,
                })
                .collect(),
            modules: modules
                .into_iter()
                .map(|m| {
                    let fallback = key_names
                        .iter()
                        .find(|(id, _)| *id == m.id)
                        .map(|(_, name)| {
                            name.strip_prefix("Модуль «")
                                .and_then(|n| n.strip_suffix('»'))
                                .unwrap_or(name)
                                .to_string()
                        });
                    MetaModule {
                        name: m.name.or(fallback).unwrap_or_else(|| m.id.clone()),
                        id: m.id,
                    }
                })
                .collect(),
            areas: Area::ALL
                .into_iter()
                .map(|area| MetaArea {
                    id: area,
                    title: area.title(),
                    actions: labels::ACTIONS
                        .iter()
                        .filter(|label| label.area == area)
                        .map(|label| MetaAction {
                            code: label.code,
                            title: label.title,
                        })
                        .collect(),
                })
                .collect(),
            search_min: SEARCH_MIN,
        })
    }
}
