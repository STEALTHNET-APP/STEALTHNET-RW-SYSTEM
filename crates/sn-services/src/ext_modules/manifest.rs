use super::address;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sn_domain::access::Permission;
use sn_domain::module::ModuleId;

pub const FORMAT: u64 = 1;
pub const MAX_BYTES: usize = 64 * 1024;
const ID_MIN: usize = 2;
const ID_MAX: usize = 40;
const NAME_MAX: usize = 60;
const VERSION_MAX: usize = 30;
const AUTHOR_MAX: usize = 80;
const DESCRIPTION_MAX: usize = 300;
const PAGE_TITLE_MAX: usize = 40;
const FIELDS_MAX: usize = 30;
const FIELD_KEY_MAX: usize = 40;
const LABEL_MAX: usize = 80;
const HINT_MAX: usize = 300;
const OPTIONS_MAX: usize = 50;
pub const TEXT_MAX: usize = 300;
pub const LARGE_TEXT_MAX: usize = 4000;
const RESERVED_IDS: [&str; 5] = ["me", "install", "ext", "packages", "install-info"];
const TOP_FIELDS: [&str; 15] = [
    "format",
    "id",
    "name",
    "version",
    "author",
    "description",
    "site",
    "webhook",
    "events",
    "permissions",
    "page",
    "client_card",
    "bot",
    "settings",
    "run",
];
const PAGE_FIELDS: [&str; 4] = ["url", "path", "title", "permission"];
const CARD_FIELDS: [&str; 3] = ["path", "title", "permission"];
const BOT_FIELDS: [&str; 2] = ["button", "path"];
pub const BUTTON_MAX: usize = 32;
const RUN_FIELDS: [&str; 9] = [
    "port",
    "events_path",
    "health_path",
    "inbound_path",
    "memory_mb",
    "cpus",
    "internet",
    "volumes",
    "images",
];
const VOLUME_FIELDS: [&str; 2] = ["name", "path"];
const IMAGE_FIELDS: [&str; 3] = ["file", "sha256", "bytes"];
pub const PLATFORMS: [(&str, &str); 2] = [
    ("linux/amd64", "image-linux-amd64.tar"),
    ("linux/arm64", "image-linux-arm64.tar"),
];
pub const PORT_MIN: u16 = 1024;
pub const MEMORY_MIN_MB: u32 = 64;
pub const MEMORY_MAX_MB: u32 = 2048;
pub const MEMORY_DEFAULT_MB: u32 = 256;
pub const CPU_MIN_TENTHS: u32 = 1;
pub const CPU_MAX_TENTHS: u32 = 20;
pub const CPU_DEFAULT_TENTHS: u32 = 10;
pub const VOLUMES_MAX: usize = 3;
pub const IMAGE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const INNER_PATH_MAX: usize = 200;
const VOLUME_NAME_MAX: usize = 16;
const RESERVED_MOUNTS: [&str; 5] = ["/proc", "/sys", "/dev", "/tmp", "/run"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    External,
    Package,
}
const SETTING_FIELDS: [&str; 9] = [
    "key", "type", "label", "hint", "required", "default", "large", "options", "labels",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventInfo {
    pub kind: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub needs: Permission,
}

const fn event(
    kind: &'static str,
    title: &'static str,
    about: &'static str,
    needs: Permission,
) -> EventInfo {
    EventInfo {
        kind,
        title,
        about,
        needs,
    }
}

pub const EVENTS: [EventInfo; 14] = [
    event(
        "client_registered",
        "Клиент зарегистрировался",
        "номер клиента, откуда он пришёл и параметр ссылки",
        Permission::ClientsRead,
    ),
    event(
        "client_blocked",
        "Клиент заблокирован",
        "номер клиента и причина",
        Permission::ClientsRead,
    ),
    event(
        "client_unblocked",
        "Клиент разблокирован",
        "номер клиента",
        Permission::ClientsRead,
    ),
    event(
        "client_telegram_linked",
        "Клиент привязал Telegram",
        "номер клиента и его Telegram ID",
        Permission::ClientsRead,
    ),
    event(
        "payment_paid",
        "Оплата прошла",
        "номер оплаты и заказа, что куплено, сумма и платёжная система",
        Permission::Money,
    ),
    event(
        "payment_refunded",
        "Оплата возвращена",
        "номер оплаты и заказа, сумма возврата",
        Permission::Money,
    ),
    event(
        "order_fulfilled",
        "Покупка выдана",
        "номер заказа, что куплено, срок в днях, докупленные устройства и номер подписки",
        Permission::ClientsRead,
    ),
    event(
        "subscription_created",
        "Подписка выдана",
        "номер подписки и клиента, вид подписки",
        Permission::ClientsRead,
    ),
    event(
        "subscription_renewed",
        "Подписка продлена",
        "номер подписки и новая дата окончания",
        Permission::ClientsRead,
    ),
    event(
        "tariff_changed",
        "Тариф сменился",
        "номер подписки, прежний и новый тариф",
        Permission::ClientsRead,
    ),
    event(
        "subscription_status_changed",
        "Статус подписки изменился",
        "активна, ограничена, истекла или отключена",
        Permission::ClientsRead,
    ),
    event(
        "balance_changed",
        "Баланс изменился",
        "номер клиента, кошелёк, причина и сумма",
        Permission::Money,
    ),
    event(
        "ticket_opened",
        "Новое обращение в поддержку",
        "номер обращения и клиента",
        Permission::Support,
    ),
    event(
        "ticket_answered",
        "Поддержка ответила клиенту",
        "номер обращения и клиента",
        Permission::Support,
    ),
];

pub fn event_info(kind: &str) -> Option<&'static EventInfo> {
    EVENTS.iter().find(|e| e.kind == kind)
}

pub const fn permission_title(permission: Permission) -> &'static str {
    match permission {
        Permission::ClientsRead => "Клиенты: просмотр",
        Permission::ClientsWrite => "Клиенты: изменения",
        Permission::Money => "Деньги",
        Permission::Refunds => "Возвраты",
        Permission::Tariffs => "Тарифы",
        Permission::Infrastructure => "Инфраструктура",
        Permission::Support => "Поддержка",
        Permission::Marketing => "Маркетинг",
        Permission::Modules => "Модули",
        Permission::Settings => "Настройки",
        Permission::Admins => "Администраторы",
        Permission::ClientsLogin => "Вход за клиента",
        Permission::RemnaSync => "Синхронизация с Remnawave",
        Permission::Backups => "Резервные копии",
        Permission::AuditLog => "Журнал действий",
    }
}

pub const fn permission_about(permission: Permission) -> &'static str {
    match permission {
        Permission::ClientsRead => "видеть клиентов, их подписки и историю",
        Permission::ClientsWrite => {
            "менять подписки и баланс клиентов, писать им, блокировать и удалять"
        }
        Permission::Money => "видеть оплаты, заказы и выручку, разбирать спорные оплаты",
        Permission::Refunds => "возвращать клиентам деньги",
        Permission::Tariffs => "менять тарифы и цены",
        Permission::Infrastructure => "управлять нодами, сквадами и хостами Remnawave",
        Permission::Support => "читать обращения и отвечать клиентам",
        Permission::Marketing => "промокоды, рассылки, рефералы, конкурсы",
        Permission::Modules => "включать и настраивать встроенные модули",
        Permission::Settings => {
            "менять настройки панели, в том числе ключи платёжных систем и бота"
        }
        Permission::Admins => "управлять администраторами",
        Permission::ClientsLogin => {
            "выдавать клиенту пароль для сайта и снимать ему вход по коду из приложения"
        }
        Permission::RemnaSync => "переносить клиентов и подписки между панелью и Remnawave",
        Permission::Backups => "делать, скачивать и восстанавливать резервные копии",
        Permission::AuditLog => "смотреть, кто, что и когда менял в панели",
    }
}

pub const fn sensitive(permission: Permission) -> bool {
    !matches!(
        permission,
        Permission::ClientsRead | Permission::Support | Permission::Marketing
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub title: String,
    pub permission: Permission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCard {
    pub path: String,
    pub title: String,
    pub permission: Permission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotEntry {
    pub button: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageFile {
    pub platform: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events_path: Option<String>,
    pub health_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_path: Option<String>,
    pub memory_mb: u32,
    pub cpu_tenths: u32,
    pub internet: bool,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub images: Vec<ImageFile>,
}

impl Run {
    pub fn image(&self, platform: &str) -> Option<&ImageFile> {
        self.images.iter().find(|i| i.platform == platform)
    }

    pub fn cpus_text(&self) -> String {
        format!("{}.{}", self.cpu_tenths / 10, self.cpu_tenths % 10)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Secret,
    Number,
    Flag,
    Url,
    Select { options: Vec<String> },
}

impl FieldKind {
    pub const fn name(&self) -> &'static str {
        match self {
            FieldKind::Text => "text",
            FieldKind::Secret => "secret",
            FieldKind::Number => "number",
            FieldKind::Flag => "flag",
            FieldKind::Url => "url",
            FieldKind::Select { .. } => "select",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub kind: FieldKind,
    pub required: bool,
    pub default: Option<String>,
    pub large: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

impl Field {
    pub const fn is_secret(&self) -> bool {
        matches!(self.kind, FieldKind::Secret)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u64,
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: Option<String>,
    pub site: Option<String>,
    pub webhook: Option<String>,
    pub events: Vec<String>,
    pub permissions: Vec<Permission>,
    pub page: Option<Page>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_card: Option<ClientCard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<BotEntry>,
    pub settings: Vec<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
}

impl Manifest {
    pub const fn packaged(&self) -> bool {
        self.run.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

#[derive(Default)]
struct Problems(Vec<Problem>);

impl Problems {
    fn add(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(Problem {
            path: path.into(),
            message: message.into(),
        });
    }
}

fn chars(value: &str) -> usize {
    value.chars().count()
}

fn clean(value: &str) -> bool {
    !value.chars().any(|c| c.is_control() && c != '\n')
}

fn text(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    max: usize,
    required: bool,
    problems: &mut Problems,
) -> Option<String> {
    match map.get(key) {
        None | Some(Value::Null) => {
            if required {
                problems.add(path, "поле обязательно");
            }
            None
        }
        Some(Value::String(raw)) => {
            let value = raw.trim();
            if value.is_empty() {
                if required {
                    problems.add(path, "поле не может быть пустым");
                }
                return None;
            }
            if chars(value) > max {
                problems.add(path, format!("не длиннее {max} символов"));
                return None;
            }
            if !clean(value) {
                problems.add(path, "в тексте есть служебные символы");
                return None;
            }
            Some(value.to_string())
        }
        Some(_) => {
            problems.add(path, "ожидается строка в кавычках");
            None
        }
    }
}

fn flag(map: &Map<String, Value>, key: &str, path: &str, problems: &mut Problems) -> bool {
    match map.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            problems.add(path, "ожидается true или false");
            false
        }
    }
}

fn unknown(map: &Map<String, Value>, known: &[&str], prefix: &str, problems: &mut Problems) {
    for key in map.keys() {
        if !known.contains(&key.as_str()) {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            problems.add(path, format!("неизвестное поле «{key}»"));
        }
    }
}

pub fn id_problem(id: &str) -> Option<String> {
    let length = id.len();
    let shape = id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && (ID_MIN..=ID_MAX).contains(&length);
    if !shape {
        return Some(format!(
            "от {ID_MIN} до {ID_MAX} символов: латинские буквы в нижнем регистре, цифры и дефис, первая — буква"
        ));
    }
    if ModuleId::parse(id).is_ok() {
        return Some(format!("«{id}» — имя встроенного модуля, выберите другое"));
    }
    if RESERVED_IDS.contains(&id) {
        return Some(format!("«{id}» занято панелью, выберите другое"));
    }
    None
}

fn format_version(map: &Map<String, Value>, problems: &mut Problems) -> u64 {
    match map.get("format") {
        None | Some(Value::Null) => {
            problems.add("format", "укажите версию формата: \"format\": 1");
            0
        }
        Some(Value::Number(n)) => match n.as_u64() {
            Some(FORMAT) => FORMAT,
            Some(other) => {
                problems.add(
                    "format",
                    format!(
                        "версия формата {other} этой панели неизвестна, поддерживается только 1"
                    ),
                );
                other
            }
            None => {
                problems.add("format", "ожидается число 1");
                0
            }
        },
        Some(_) => {
            problems.add("format", "ожидается число 1");
            0
        }
    }
}

fn permission(raw: &str) -> Result<Permission, String> {
    match Permission::parse(raw) {
        Ok(Permission::Admins) => Err("право «Администраторы» модулю дать нельзя".into()),
        Ok(Permission::ClientsLogin) => Err("право «Вход за клиента» модулю дать нельзя".into()),
        Ok(Permission::RemnaSync) => {
            Err("право «Синхронизация с Remnawave» модулю дать нельзя".into())
        }
        Ok(Permission::Backups) => Err("право «Резервные копии» модулю дать нельзя".into()),
        Ok(Permission::AuditLog) => Err("право «Журнал действий» модулю дать нельзя".into()),
        Ok(permission) => Ok(permission),
        Err(_) => Err(format!(
            "неизвестное право «{raw}». Есть: {}",
            Permission::ALL
                .iter()
                .filter(|p| {
                    !matches!(
                        p,
                        Permission::Admins
                            | Permission::ClientsLogin
                            | Permission::RemnaSync
                            | Permission::Backups
                            | Permission::AuditLog
                    )
                })
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn string_list(
    map: &Map<String, Value>,
    key: &str,
    problems: &mut Problems,
) -> Vec<(String, String)> {
    match map.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                Value::String(s) => Some((format!("{key}[{i}]"), s.trim().to_string())),
                _ => {
                    problems.add(format!("{key}[{i}]"), "ожидается строка в кавычках");
                    None
                }
            })
            .collect(),
        Some(_) => {
            problems.add(key, "ожидается список в квадратных скобках [ … ]");
            Vec::new()
        }
    }
}

fn permissions(map: &Map<String, Value>, problems: &mut Problems) -> Vec<Permission> {
    let mut out: Vec<Permission> = Vec::new();
    for (path, raw) in string_list(map, "permissions", problems) {
        match permission(&raw) {
            Ok(p) if !out.contains(&p) => out.push(p),
            Ok(_) => {}
            Err(message) => problems.add(path, message),
        }
    }
    out.sort();
    out
}

fn events(
    map: &Map<String, Value>,
    granted: &[Permission],
    problems: &mut Problems,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, raw) in string_list(map, "events", problems) {
        let Some(info) = event_info(&raw) else {
            problems.add(
                path,
                format!(
                    "неизвестное событие «{raw}». Есть: {}",
                    EVENTS.iter().map(|e| e.kind).collect::<Vec<_>>().join(", ")
                ),
            );
            continue;
        };
        if !granted.contains(&info.needs) {
            problems.add(
                path,
                format!(
                    "событие «{}» приходит только с правом «{}»: добавьте \"{}\" в permissions",
                    info.title,
                    permission_title(info.needs),
                    info.needs.as_str()
                ),
            );
        }
        if !out.iter().any(|e| e == info.kind) {
            out.push(info.kind.to_string());
        }
    }
    out.sort_by_key(|kind| EVENTS.iter().position(|e| e.kind == kind));
    out
}

fn url(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    required: bool,
    problems: &mut Problems,
) -> Option<String> {
    let raw = text(map, key, path, address::URL_MAX, required, problems)?;
    match address::check(&raw) {
        Ok(url) => Some(url.to_string()),
        Err(message) => {
            problems.add(path, message);
            None
        }
    }
}

pub fn inner_path_problem(raw: &str) -> Option<&'static str> {
    if !raw.starts_with('/') {
        return Some("путь внутри модуля начинается с /, например /events");
    }
    if raw.len() > INNER_PATH_MAX {
        return Some("путь не длиннее 200 символов");
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~'))
    {
        return Some("в пути только латинские буквы, цифры и знаки / - _ . ~");
    }
    if raw.contains("//") || raw.split('/').any(|part| part == "." || part == "..") {
        return Some("в пути не бывает //, . и ..");
    }
    None
}

fn inner_path(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    required: bool,
    problems: &mut Problems,
) -> Option<String> {
    let raw = text(map, key, path, INNER_PATH_MAX + 1, required, problems)?;
    match inner_path_problem(&raw) {
        Some(message) => {
            problems.add(path, message);
            None
        }
        None => Some(raw),
    }
}

fn page(map: &Map<String, Value>, name: &str, kind: Kind, problems: &mut Problems) -> Option<Page> {
    let value = map.get("page")?;
    if value.is_null() {
        return None;
    }
    let Value::Object(page) = value else {
        problems.add(
            "page",
            "ожидается объект { \"url\": …, \"title\": … } в фигурных скобках",
        );
        return None;
    };
    unknown(page, &PAGE_FIELDS, "page", problems);
    let (url, path) = match kind {
        Kind::External => {
            if page.contains_key("path") {
                problems.add(
                    "page.path",
                    "page.path бывает только в архиве модуля, для модуля на своём сервере укажите page.url",
                );
            }
            (url(page, "url", "page.url", true, problems), None)
        }
        Kind::Package => {
            if page.contains_key("url") {
                problems.add(
                    "page.url",
                    "в архиве модуля адреса страницы нет: укажите page.path внутри контейнера, например \"/\"",
                );
            }
            match inner_path(page, "path", "page.path", true, problems) {
                Some(path) => (Some(String::new()), Some(path)),
                None => (None, None),
            }
        }
    };
    let title = text(page, "title", "page.title", PAGE_TITLE_MAX, false, problems)
        .unwrap_or_else(|| name.chars().take(PAGE_TITLE_MAX).collect());
    let permission = match page.get("permission") {
        None | Some(Value::Null) => Some(Permission::Modules),
        Some(Value::String(raw)) => match self::permission(raw.trim()) {
            Ok(p) => Some(p),
            Err(message) => {
                problems.add("page.permission", message);
                None
            }
        },
        Some(_) => {
            problems.add("page.permission", "ожидается строка в кавычках");
            None
        }
    };
    Some(Page {
        url: url?,
        path,
        title,
        permission: permission?,
    })
}

fn package_only(
    map: &Map<String, Value>,
    key: &str,
    kind: Kind,
    what: &str,
    problems: &mut Problems,
) -> Option<Map<String, Value>> {
    let value = map.get(key)?;
    if value.is_null() {
        return None;
    }
    if kind == Kind::External {
        problems.add(
            key,
            format!("{what} бывает только у модуля из архива: соберите zip командой stealthnet module pack"),
        );
        return None;
    }
    match value {
        Value::Object(inner) => Some(inner.clone()),
        _ => {
            problems.add(key, "ожидается объект в фигурных скобках { … }");
            None
        }
    }
}

fn client_card(
    map: &Map<String, Value>,
    name: &str,
    kind: Kind,
    problems: &mut Problems,
) -> Option<ClientCard> {
    let card = package_only(
        map,
        "client_card",
        kind,
        "Раздел в карточке клиента",
        problems,
    )?;
    unknown(&card, &CARD_FIELDS, "client_card", problems);
    let path = inner_path(&card, "path", "client_card.path", true, problems);
    let title = text(
        &card,
        "title",
        "client_card.title",
        PAGE_TITLE_MAX,
        false,
        problems,
    )
    .unwrap_or_else(|| name.chars().take(PAGE_TITLE_MAX).collect());
    let permission = match card.get("permission") {
        None | Some(Value::Null) => Some(Permission::ClientsRead),
        Some(Value::String(raw)) => match self::permission(raw.trim()) {
            Ok(p) => Some(p),
            Err(message) => {
                problems.add("client_card.permission", message);
                None
            }
        },
        Some(_) => {
            problems.add("client_card.permission", "ожидается строка в кавычках");
            None
        }
    };
    Some(ClientCard {
        path: path?,
        title,
        permission: permission?,
    })
}

fn bot_entry(
    map: &Map<String, Value>,
    kind: Kind,
    granted: &[Permission],
    problems: &mut Problems,
) -> Option<BotEntry> {
    let bot = package_only(map, "bot", kind, "Кнопка в боте", problems)?;
    unknown(&bot, &BOT_FIELDS, "bot", problems);
    let button = text(&bot, "button", "bot.button", BUTTON_MAX, true, problems).filter(|button| {
        let ok = !button.contains('\n');
        if !ok {
            problems.add("bot.button", "надпись на кнопке — одна строка");
        }
        ok
    });
    let path = inner_path(&bot, "path", "bot.path", true, problems);
    if !granted.contains(&Permission::ClientsRead) {
        problems.add(
            "bot",
            "экран в боте получает номер клиента и его Telegram ID: добавьте \"clients_read\" в permissions",
        );
    }
    Some(BotEntry {
        button: button?,
        path: path?,
    })
}

fn whole(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    range: std::ops::RangeInclusive<u64>,
    problems: &mut Problems,
) -> Option<u64> {
    match map.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => match n.as_u64().filter(|v| range.contains(v)) {
            Some(v) => Some(v),
            None => {
                problems.add(
                    path,
                    format!(
                        "ожидается целое число от {} до {}",
                        range.start(),
                        range.end()
                    ),
                );
                None
            }
        },
        Some(_) => {
            problems.add(path, "ожидается число без кавычек");
            None
        }
    }
}

fn cpus(map: &Map<String, Value>, problems: &mut Problems) -> Option<u32> {
    let value = map.get("cpus")?;
    let parsed = value.as_f64().filter(|f| f.is_finite()).and_then(|f| {
        let tenths = (f * 10.0).round();
        ((f * 10.0 - tenths).abs() < 1e-6
            && (f64::from(CPU_MIN_TENTHS)..=f64::from(CPU_MAX_TENTHS)).contains(&tenths))
        .then_some(tenths as u32)
    });
    if parsed.is_none() {
        problems.add(
            "run.cpus",
            "ожидается число ядер от 0.1 до 2 с шагом 0.1, например 0.5",
        );
    }
    parsed
}

fn volumes(map: &Map<String, Value>, problems: &mut Problems) -> Vec<Volume> {
    let items = match map.get("volumes") {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => {
            problems.add("run.volumes", "ожидается список в квадратных скобках [ … ]");
            return Vec::new();
        }
    };
    if items.len() > VOLUMES_MAX {
        problems.add(
            "run.volumes",
            format!("не больше {VOLUMES_MAX} папок данных"),
        );
        return Vec::new();
    }
    let mut out: Vec<Volume> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("run.volumes[{i}]");
        let Value::Object(volume) = item else {
            problems.add(&path, "ожидается объект { \"name\": …, \"path\": … }");
            continue;
        };
        unknown(volume, &VOLUME_FIELDS, &path, problems);
        let name = text(
            volume,
            "name",
            &format!("{path}.name"),
            VOLUME_NAME_MAX + 10,
            true,
            problems,
        )
        .filter(|name| {
            let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
                && name.len() <= VOLUME_NAME_MAX
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
            if !ok {
                problems.add(
                    format!("{path}.name"),
                    "до 16 символов: латинские буквы в нижнем регистре и цифры, первая — буква",
                );
            }
            ok
        });
        let mount = inner_path(volume, "path", &format!("{path}.path"), true, problems).filter(
            |mount| {
                let trimmed = mount.trim_end_matches('/');
                let bad = trimmed.is_empty()
                    || RESERVED_MOUNTS
                        .iter()
                        .any(|r| trimmed == *r || trimmed.starts_with(&format!("{r}/")));
                if bad {
                    problems.add(
                        format!("{path}.path"),
                        "нельзя /, /proc, /sys, /dev, /tmp и /run: выберите свою папку, например /data",
                    );
                }
                !bad
            },
        );
        let (Some(name), Some(mount)) = (name, mount) else {
            continue;
        };
        let mount = mount.trim_end_matches('/').to_string();
        if out.iter().any(|v| v.name == name) {
            problems.add(format!("{path}.name"), format!("папка «{name}» уже есть"));
            continue;
        }
        if out.iter().any(|v| {
            v.path == mount
                || v.path.starts_with(&format!("{mount}/"))
                || mount.starts_with(&format!("{}/", v.path))
        }) {
            problems.add(
                format!("{path}.path"),
                "папки данных не могут совпадать или лежать одна в другой",
            );
            continue;
        }
        out.push(Volume { name, path: mount });
    }
    out
}

fn images(map: &Map<String, Value>, problems: &mut Problems) -> Vec<ImageFile> {
    let items = match map.get("images") {
        None | Some(Value::Null) => {
            problems.add(
                "run.images",
                "укажите образы модуля: соберите архив командой stealthnet module pack",
            );
            return Vec::new();
        }
        Some(Value::Object(items)) => items,
        Some(_) => {
            problems.add(
                "run.images",
                "ожидается объект { \"linux/amd64\": { … } } в фигурных скобках",
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (platform, item) in items {
        let path = format!("run.images.{platform}");
        let Some((_, expected)) = PLATFORMS.iter().find(|(p, _)| p == platform) else {
            problems.add(
                &path,
                format!(
                    "неизвестная платформа «{platform}». Есть: {}",
                    PLATFORMS
                        .iter()
                        .map(|(p, _)| *p)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            continue;
        };
        let Value::Object(image) = item else {
            problems.add(
                &path,
                "ожидается объект { \"file\": …, \"sha256\": …, \"bytes\": … }",
            );
            continue;
        };
        unknown(image, &IMAGE_FIELDS, &path, problems);
        let file =
            text(image, "file", &format!("{path}.file"), 80, true, problems).filter(|file| {
                let ok = file == expected;
                if !ok {
                    problems.add(
                        format!("{path}.file"),
                        format!("файл образа для {platform} называется {expected}"),
                    );
                }
                ok
            });
        let sha256 = text(
            image,
            "sha256",
            &format!("{path}.sha256"),
            80,
            true,
            problems,
        )
        .filter(|sum| {
            let ok = sum.len() == 64
                && sum
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !ok {
                problems.add(
                    format!("{path}.sha256"),
                    "ожидается sha256: 64 знака 0-9 и a-f",
                );
            }
            ok
        });
        let bytes = whole(
            image,
            "bytes",
            &format!("{path}.bytes"),
            1..=IMAGE_MAX_BYTES,
            problems,
        );
        if image.get("bytes").is_none_or(Value::is_null) {
            problems.add(format!("{path}.bytes"), "поле обязательно");
        }
        if let (Some(file), Some(sha256), Some(bytes)) = (file, sha256, bytes) {
            out.push(ImageFile {
                platform: platform.clone(),
                file,
                sha256,
                bytes,
            });
        }
    }
    if out.is_empty() && problems.0.iter().all(|p| !p.path.starts_with("run.images")) {
        problems.add("run.images", "в архиве нет ни одного образа");
    }
    out.sort_by_key(|i| PLATFORMS.iter().position(|(p, _)| *p == i.platform));
    out
}

fn run(map: &Map<String, Value>, kind: Kind, problems: &mut Problems) -> Option<Run> {
    let value = map.get("run");
    match kind {
        Kind::External => {
            if value.is_some_and(|v| !v.is_null()) {
                problems.add(
                    "run",
                    "раздел run бывает только в архиве модуля: загрузите zip, собранный командой stealthnet module pack",
                );
            }
            return None;
        }
        Kind::Package => {}
    }
    let run = match value {
        None | Some(Value::Null) => {
            problems.add(
                "run",
                "в архиве модуля нужен раздел run: порт, проверка работы, память и образы",
            );
            return None;
        }
        Some(Value::Object(run)) => run,
        Some(_) => {
            problems.add(
                "run",
                "ожидается объект { \"port\": …, … } в фигурных скобках",
            );
            return None;
        }
    };
    unknown(run, &RUN_FIELDS, "run", problems);
    let port = whole(
        run,
        "port",
        "run.port",
        u64::from(PORT_MIN)..=u64::from(u16::MAX),
        problems,
    );
    if run.get("port").is_none_or(Value::is_null) {
        problems.add(
            "run.port",
            "укажите порт, на котором модуль слушает внутри контейнера",
        );
    }
    let events_path = inner_path(run, "events_path", "run.events_path", false, problems);
    let health_path = inner_path(run, "health_path", "run.health_path", true, problems);
    let inbound_path = inner_path(run, "inbound_path", "run.inbound_path", false, problems);
    let memory_mb = whole(
        run,
        "memory_mb",
        "run.memory_mb",
        u64::from(MEMORY_MIN_MB)..=u64::from(MEMORY_MAX_MB),
        problems,
    )
    .map(|m| m as u32)
    .unwrap_or(MEMORY_DEFAULT_MB);
    let cpu_tenths = cpus(run, problems).unwrap_or(CPU_DEFAULT_TENTHS);
    let internet = flag(run, "internet", "run.internet", problems);
    let volumes = volumes(run, problems);
    let images = images(run, problems);
    Some(Run {
        port: u16::try_from(port?).ok()?,
        events_path,
        health_path: health_path?,
        inbound_path,
        memory_mb,
        cpu_tenths,
        internet,
        volumes,
        images,
    })
}

fn default_value(
    field: &Map<String, Value>,
    kind: &FieldKind,
    path: &str,
    problems: &mut Problems,
) -> Option<String> {
    let value = field.get("default")?;
    let path = format!("{path}.default");
    let rendered = match (kind, value) {
        (_, Value::Null) => return None,
        (FieldKind::Secret, _) => {
            problems.add(path, "у секретного поля не бывает значения по умолчанию");
            return None;
        }
        (FieldKind::Flag, Value::Bool(b)) => b.to_string(),
        (FieldKind::Flag, _) => {
            problems.add(path, "ожидается true или false");
            return None;
        }
        (FieldKind::Number, Value::Number(n)) => n.to_string(),
        (FieldKind::Number, Value::String(s)) if number(s).is_some() => s.trim().to_string(),
        (FieldKind::Number, _) => {
            problems.add(path, "ожидается число");
            return None;
        }
        (_, Value::String(s)) => s.trim().to_string(),
        _ => {
            problems.add(path, "ожидается строка в кавычках");
            return None;
        }
    };
    match kind {
        FieldKind::Select { options } if !options.contains(&rendered) => {
            problems.add(path, "значение по умолчанию должно быть из options");
            None
        }
        FieldKind::Url if !rendered.is_empty() && !address::setting_url(&rendered) => {
            problems.add(path, "ожидается адрес https://…");
            None
        }
        FieldKind::Text if chars(&rendered) > LARGE_TEXT_MAX => {
            problems.add(path, format!("не длиннее {LARGE_TEXT_MAX} символов"));
            None
        }
        _ => Some(rendered),
    }
}

pub fn number(raw: &str) -> Option<serde_json::Number> {
    let raw = raw.trim();
    if let Ok(whole) = raw.parse::<i64>() {
        return Some(whole.into());
    }
    raw.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .and_then(serde_json::Number::from_f64)
}

fn field(item: &Value, path: &str, problems: &mut Problems) -> Option<Field> {
    let Value::Object(field) = item else {
        problems.add(path, "ожидается объект { \"key\": …, \"type\": … }");
        return None;
    };
    unknown(field, &SETTING_FIELDS, path, problems);
    let key = text(
        field,
        "key",
        &format!("{path}.key"),
        FIELD_KEY_MAX,
        true,
        problems,
    );
    let key = key.filter(|key| {
        let ok = key.starts_with(|c: char| c.is_ascii_lowercase())
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            problems.add(
                format!("{path}.key"),
                "латинские буквы в нижнем регистре, цифры и _, первая — буква",
            );
        }
        ok
    });
    let label = text(
        field,
        "label",
        &format!("{path}.label"),
        LABEL_MAX,
        true,
        problems,
    );
    let hint = text(
        field,
        "hint",
        &format!("{path}.hint"),
        HINT_MAX,
        false,
        problems,
    )
    .unwrap_or_default();
    let required = flag(field, "required", &format!("{path}.required"), problems);
    let large = flag(field, "large", &format!("{path}.large"), problems);
    let kind = match field.get("type") {
        Some(Value::String(t)) => match t.trim() {
            "text" => Some(FieldKind::Text),
            "secret" => Some(FieldKind::Secret),
            "number" => Some(FieldKind::Number),
            "flag" => Some(FieldKind::Flag),
            "url" => Some(FieldKind::Url),
            "select" => {
                let options = string_list(field, "options", problems);
                if options.is_empty() || options.len() > OPTIONS_MAX {
                    problems.add(
                        format!("{path}.options"),
                        format!("у списка выбора от 1 до {OPTIONS_MAX} вариантов"),
                    );
                }
                let mut values: Vec<String> = Vec::new();
                for (_, option) in options {
                    if option.is_empty() || chars(&option) > LABEL_MAX || !clean(&option) {
                        problems.add(
                            format!("{path}.options"),
                            format!("вариант — непустая строка до {LABEL_MAX} символов"),
                        );
                    } else if values.contains(&option) {
                        problems.add(
                            format!("{path}.options"),
                            format!("вариант «{option}» повторяется"),
                        );
                    } else {
                        values.push(option);
                    }
                }
                Some(FieldKind::Select { options: values })
            }
            other => {
                problems.add(
                    format!("{path}.type"),
                    format!(
                        "неизвестный тип «{other}». Есть: text, secret, number, flag, url, select"
                    ),
                );
                None
            }
        },
        None | Some(Value::Null) => {
            problems.add(format!("{path}.type"), "поле обязательно");
            None
        }
        Some(_) => {
            problems.add(format!("{path}.type"), "ожидается строка в кавычках");
            None
        }
    };
    let kind = kind?;
    if large && kind != FieldKind::Text {
        problems.add(
            format!("{path}.large"),
            "большое поле бывает только у type: text",
        );
    }
    let labels = match (&kind, field.get("labels")) {
        (_, None | Some(Value::Null)) => None,
        (FieldKind::Select { options }, Some(_)) => {
            let labels: Vec<String> = string_list(field, "labels", problems)
                .into_iter()
                .map(|(_, l)| l)
                .collect();
            if labels.len() != options.len()
                || labels
                    .iter()
                    .any(|l| l.is_empty() || chars(l) > LABEL_MAX || !clean(l))
            {
                problems.add(
                    format!("{path}.labels"),
                    "подписей должно быть столько же, сколько вариантов, каждая до 80 символов",
                );
                None
            } else {
                Some(labels)
            }
        }
        (_, Some(_)) => {
            problems.add(
                format!("{path}.labels"),
                "подписи вариантов бывают только у type: select",
            );
            None
        }
    };
    let default = default_value(field, &kind, path, problems);
    Some(Field {
        key: key?,
        label: label?,
        hint,
        required: required && !matches!(kind, FieldKind::Flag),
        large: large && kind == FieldKind::Text,
        kind,
        default,
        labels,
    })
}

fn settings(map: &Map<String, Value>, problems: &mut Problems) -> Vec<Field> {
    let items = match map.get("settings") {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => {
            problems.add(
                "settings",
                "ожидается список полей в квадратных скобках [ … ]",
            );
            return Vec::new();
        }
    };
    if items.len() > FIELDS_MAX {
        problems.add("settings", format!("не больше {FIELDS_MAX} полей"));
        return Vec::new();
    }
    let mut out: Vec<Field> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = format!("settings[{i}]");
        if let Some(field) = field(item, &path, problems) {
            if out.iter().any(|f| f.key == field.key) {
                problems.add(
                    format!("{path}.key"),
                    format!("поле «{}» уже есть", field.key),
                );
            } else {
                out.push(field);
            }
        }
    }
    out
}

pub fn parse(source: &str) -> Result<Manifest, Vec<Problem>> {
    parse_as(source, Kind::External)
}

pub fn parse_package(source: &str) -> Result<Manifest, Vec<Problem>> {
    parse_as(source, Kind::Package)
}

pub fn parse_as(source: &str, kind: Kind) -> Result<Manifest, Vec<Problem>> {
    let mut problems = Problems::default();
    let source = source.trim_start_matches('\u{feff}');
    if source.trim().is_empty() {
        problems.add("", "файл пустой");
        return Err(problems.0);
    }
    if source.len() > MAX_BYTES {
        problems.add("", "файл больше 64 КБ — это не похоже на описание модуля");
        return Err(problems.0);
    }
    let value: Value = match serde_json::from_str(source) {
        Ok(value) => value,
        Err(err) => {
            problems.add(
                "",
                format!(
                    "это не JSON: ошибка в строке {}, столбце {}",
                    err.line(),
                    err.column()
                ),
            );
            return Err(problems.0);
        }
    };
    let Value::Object(map) = value else {
        problems.add("", "описание модуля — это объект в фигурных скобках { … }");
        return Err(problems.0);
    };
    unknown(&map, &TOP_FIELDS, "", &mut problems);
    let format = format_version(&map, &mut problems);
    if format != FORMAT {
        return Err(problems.0);
    }
    let id =
        text(&map, "id", "id", ID_MAX + 10, true, &mut problems).filter(|id| {
            match id_problem(id) {
                Some(message) => {
                    problems.add("id", message);
                    false
                }
                None => true,
            }
        });
    let name = text(&map, "name", "name", NAME_MAX, true, &mut problems);
    let version = text(&map, "version", "version", VERSION_MAX, true, &mut problems);
    let author = text(&map, "author", "author", AUTHOR_MAX, true, &mut problems);
    let description = text(
        &map,
        "description",
        "description",
        DESCRIPTION_MAX,
        false,
        &mut problems,
    );
    let site = url(&map, "site", "site", false, &mut problems);
    let permissions = permissions(&map, &mut problems);
    let events = events(&map, &permissions, &mut problems);
    let webhook = match kind {
        Kind::External => {
            let found = problems.0.len();
            let webhook = url(&map, "webhook", "webhook", false, &mut problems);
            if webhook.is_none() && !events.is_empty() && problems.0.len() == found {
                problems.add(
                    "webhook",
                    "укажите адрес, куда слать события: модуль подписан на события",
                );
            }
            webhook
        }
        Kind::Package => {
            if map.get("webhook").is_some_and(|v| !v.is_null()) {
                problems.add(
                    "webhook",
                    "в архиве модуля адреса нет: события приходят на run.events_path внутри контейнера",
                );
            }
            None
        }
    };
    let page = page(
        &map,
        name.as_deref().unwrap_or_default(),
        kind,
        &mut problems,
    );
    let client_card = client_card(
        &map,
        name.as_deref().unwrap_or_default(),
        kind,
        &mut problems,
    );
    let bot = bot_entry(&map, kind, &permissions, &mut problems);
    let settings = settings(&map, &mut problems);
    let run = run(&map, kind, &mut problems);
    if kind == Kind::Package
        && !events.is_empty()
        && run.as_ref().is_some_and(|r| r.events_path.is_none())
    {
        problems.add(
            "run.events_path",
            "модуль подписан на события: укажите путь, куда их слать, например \"/events\"",
        );
    }
    if !problems.0.is_empty() {
        return Err(problems.0);
    }
    match (id, name, version, author) {
        (Some(id), Some(name), Some(version), Some(author)) => Ok(Manifest {
            format,
            id,
            name,
            version,
            author,
            description,
            site,
            webhook,
            events,
            permissions,
            page,
            client_card,
            bot,
            settings,
            run,
        }),
        _ => {
            problems.add("", "описание модуля не разобрано");
            Err(problems.0)
        }
    }
}
