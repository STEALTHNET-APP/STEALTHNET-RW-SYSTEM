pub mod archive;
pub mod crypt;
pub mod dump;
pub mod format;
pub mod load;
pub mod s3;
pub mod schedule;
pub mod sealed;

#[cfg(test)]
mod tests;

use crate::Actor;
use crate::auth::RequestMeta;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use crate::telegram;
use archive::{ArchiveError, Kind, Visitor};
use chrono::{DateTime, Utc};
use crypt::KdfParams;
use format::{AppInfo, Counts, KeyInfo, Manifest, NameInfo, RemnaInfo, RunKind};
use load::{CurrentKey, LoadPlan, Stage};
use s3::S3Place;
use schedule::{Due, Schedule};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::alerts::AlertKind;
use sn_domain::ids::AdminId;
use sn_store::{AuditEntry, Db, SecretBox};
use sqlx::PgConnection;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

pub const CONFIG_KEY: &str = "backups.config";
pub const PASSWORD_KEY: &str = "backups.password";
pub const S3_SECRET_KEY: &str = "backups.s3.secret_key";
pub const ALERTS_CURSOR: &str = "alerts.cursor.backups";
pub const PAGE_PATH: &str = "/admin/settings/backups";
pub const REMNA_SYNC_PATH: &str = "/admin/settings/remnawave/sync";
pub const BUSY: &str = "backups.busy";
pub const NO_DIR: &str = "backups.no_dir";
pub const UPLOAD_MAX: u64 = 16 * 1024 * 1024 * 1024;
const KEEP_DEFAULT: u32 = 7;
const KEEP_MAX: u32 = 100;
const PASSWORD_MIN: usize = 10;
const PASSWORD_MAX: usize = 256;
const DRAIN: std::time::Duration = std::time::Duration::from_secs(30);
const UPLOAD_TTL_HOURS: i64 = 24;
const STALE_RUN_HOURS: i64 = 6;
const TELEGRAM_TRIES: u32 = 3;
const REMNA_PAGE: u32 = 500;
const LIST_MAX: usize = 200;
const LOCK_CLASS: i32 = 0x5B4B_0001;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "to", rename_all = "snake_case")]
pub enum TelegramTarget {
    #[default]
    Off,
    Alerts,
    Chat {
        chat_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Settings {
    pub on: bool,
    #[serde(flatten)]
    pub place: S3Place,
}

fn default_keep() -> u32 {
    KEEP_DEFAULT
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupConfig {
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub schedule_since: Option<DateTime<Utc>>,
    #[serde(default = "default_keep")]
    pub keep: u32,
    #[serde(default)]
    pub telegram: TelegramTarget,
    #[serde(default)]
    pub s3: Option<S3Settings>,
    #[serde(default)]
    pub password_set_at: Option<DateTime<Utc>>,
}

impl Default for BackupConfig {
    fn default() -> Self {
        BackupConfig {
            schedule: Schedule::default(),
            schedule_since: None,
            keep: KEEP_DEFAULT,
            telegram: TelegramTarget::Off,
            s3: None,
            password_set_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackupEnv {
    pub dir: Option<PathBuf>,
    pub host_dir: Option<String>,
    pub telegram_api: String,
    pub secret_key: Option<String>,
    pub kdf: KdfParams,
    pub upload_dir: PathBuf,
    pub telegram_part: u64,
    pub container: bool,
}

impl Default for BackupEnv {
    fn default() -> Self {
        BackupEnv {
            dir: None,
            host_dir: None,
            telegram_api: "https://api.telegram.org".into(),
            secret_key: None,
            kdf: KdfParams::STRONG,
            upload_dir: std::env::temp_dir().join("stealthnet-backup-uploads"),
            telegram_part: telegram::PART_SIZE,
            container: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub kind: &'static str,
    pub state: &'static str,
    pub stage: &'static str,
    pub detail: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub result: Option<Value>,
    #[serde(skip)]
    pub watch: Option<String>,
}

pub const DOWNLOAD_CONFIRM: &str = "backups.download_confirm";
pub const DOWNLOAD_TICKET_SECONDS: i64 = 120;

#[derive(Debug, Clone)]
struct DownloadTicket {
    token: String,
    name: String,
    admin: AdminId,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct Runtime {
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    job: Mutex<Option<Job>>,
    wake: tokio::sync::Notify,
    tickets: Mutex<Vec<DownloadTicket>>,
}

impl Runtime {
    fn tickets(&self) -> std::sync::MutexGuard<'_, Vec<DownloadTicket>> {
        match self.tickets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn set_job(&self, job: Option<Job>) {
        match self.job.lock() {
            Ok(mut guard) => *guard = job,
            Err(poisoned) => *poisoned.into_inner() = job,
        }
    }

    fn update(&self, f: impl FnOnce(&mut Job)) {
        let mut guard = match self.job.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(job) = guard.as_mut() {
            f(job);
        }
    }

    pub fn job(&self) -> Option<Job> {
        match self.job.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub async fn woken(&self) {
        self.wake.notified().await;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct S3View {
    pub on: bool,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub prefix: String,
    pub access_key: String,
    pub virtual_host: bool,
    pub secret_set: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigView {
    pub schedule: Schedule,
    pub keep: u32,
    pub telegram: TelegramTarget,
    pub s3: Option<S3View>,
    pub password_set: bool,
    pub password_set_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageView {
    pub configured: bool,
    pub writable: bool,
    pub host_dir: Option<String>,
    pub used_bytes: u64,
    pub free_bytes: Option<u64>,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub title: Option<String>,
    pub forum: bool,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Delivery {
    pub state: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub parts: Option<u64>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CopyView {
    pub name: String,
    pub size: u64,
    pub created_at: DateTime<Utc>,
    pub kind: RunKind,
    pub sealed: bool,
    pub key_included: Option<bool>,
    pub counts: Option<Counts>,
    pub app: Option<String>,
    pub telegram: Option<Delivery>,
    pub s3: Option<Delivery>,
    pub by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: i64,
    pub kind: RunKind,
    pub state: String,
    pub file: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub details: Value,
    pub by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupsView {
    pub config: ConfigView,
    pub storage: StorageView,
    pub secret_key: bool,
    pub bot: bool,
    pub group: Option<GroupView>,
    pub copies: Vec<CopyView>,
    pub next_at: Option<DateTime<Utc>>,
    pub job: Option<Job>,
    pub last: Option<RunView>,
    pub restores: Vec<RunView>,
    pub offset_minutes: i32,
    pub remna_sync_path: &'static str,
    pub totp_required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigInput {
    pub schedule: Schedule,
    pub keep: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct S3Input {
    pub on: bool,
    pub endpoint: String,
    #[serde(default)]
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
    pub access_key: String,
    #[serde(default)]
    pub secret_key: Option<String>,
    #[serde(default)]
    pub virtual_host: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    File { name: String },
    Upload { id: Uuid },
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub source: String,
    pub size: u64,
    pub sealed: bool,
    pub created_at: DateTime<Utc>,
    pub kind: RunKind,
    pub app: String,
    pub current_app: String,
    pub same_version: bool,
    pub migrations_to_apply: usize,
    pub counts: Counts,
    pub tables: usize,
    pub rows: u64,
    pub remnawave: Option<RemnaInfo>,
    pub brand: Option<String>,
    pub public_url: Option<String>,
    pub keys: &'static str,
    pub secrets_in_copy: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UploadView {
    pub id: Uuid,
    pub size: u64,
    pub sealed: bool,
    pub name: String,
}

pub struct Inspection {
    pub manifest: Manifest,
    pub kind: Kind,
    pub size: u64,
    pub sha256: String,
    pub key_in_file: bool,
    pub remna_lines: u64,
}

struct Check {
    manifest: Option<Manifest>,
    rows: BTreeMap<usize, u64>,
    key: bool,
    remna_lines: u64,
}

impl Visitor for Check {
    fn manifest(&mut self, raw: &[u8]) -> io::Result<()> {
        let manifest: Manifest = serde_json::from_slice(raw).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "описание копии не читается")
        })?;
        if manifest.format != format::FORMAT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "это не копия STEALTHNET",
            ));
        }
        self.manifest = Some(manifest);
        Ok(())
    }

    fn table(&mut self, index: usize, chunk: &[u8]) -> io::Result<()> {
        *self.rows.entry(index).or_default() +=
            chunk.iter().filter(|b| **b == b'\n').count() as u64;
        Ok(())
    }

    fn table_end(&mut self, index: usize) -> io::Result<()> {
        self.rows.entry(index).or_default();
        Ok(())
    }

    fn remna(&mut self, chunk: &[u8]) -> io::Result<()> {
        self.remna_lines += chunk.iter().filter(|b| **b == b'\n').count() as u64;
        Ok(())
    }

    fn secret_key(&mut self, _raw: &[u8]) -> io::Result<()> {
        self.key = true;
        Ok(())
    }
}

pub fn inspect_file(path: &Path, password: Option<&str>) -> Result<Inspection, ArchiveError> {
    let mut check = Check {
        manifest: None,
        rows: BTreeMap::new(),
        key: false,
        remna_lines: 0,
    };
    let info = archive::read(path, password, &mut check)?;
    let manifest = check.manifest.ok_or(ArchiveError::Foreign)?;
    if check.rows.len() != manifest.tables.len() {
        return Err(ArchiveError::Damaged("в копии не хватает таблиц".into()));
    }
    for (index, table) in manifest.tables.iter().enumerate() {
        if check.rows.get(&index).copied() != Some(table.rows) {
            return Err(ArchiveError::Damaged(format!(
                "в таблице {} не то число строк",
                table.name
            )));
        }
    }
    if manifest.secret_key.included != check.key {
        return Err(ArchiveError::Damaged("ключ шифрования не на месте".into()));
    }
    Ok(Inspection {
        kind: info.kind,
        size: info.file_size,
        sha256: info.file_sha256,
        key_in_file: check.key,
        remna_lines: check.remna_lines.saturating_sub(1),
        manifest,
    })
}

fn internal(err: impl std::fmt::Display) -> ServiceError {
    ServiceError::Internal(err.to_string())
}

fn rejected(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::rejected(code, message)
}

fn clean_password(value: &str) -> Result<String, ServiceError> {
    let count = value.chars().count();
    if count < PASSWORD_MIN {
        return Err(rejected(
            "backups.password_short",
            format!("Пароль копий — не короче {PASSWORD_MIN} символов"),
        ));
    }
    if count > PASSWORD_MAX || value.chars().any(char::is_control) {
        return Err(rejected(
            "backups.password_invalid",
            "Такой пароль не подходит",
        ));
    }
    Ok(value.to_string())
}

fn free_space(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

fn audit_entry(
    actor: Actor,
    action: &str,
    details: Value,
    meta: Option<&RequestMeta>,
) -> AuditEntry {
    let mut entry = actor.audit(action, "backup", None, details);
    entry.ip = meta.and_then(|m| m.ip.clone());
    entry
}

pub fn kind_title(kind: RunKind) -> &'static str {
    match kind {
        RunKind::Auto => "по расписанию",
        RunKind::Manual => "вручную",
        RunKind::BeforeRestore => "перед восстановлением",
        RunKind::Restore => "восстановление",
    }
}

pub use sn_domain::format::utc_label;

pub fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn plural(n: u64, one: &str, few: &str, many: &str) -> String {
    format!(
        "{} {}",
        sn_domain::format::group_digits(n),
        sn_domain::format::plural_ru(n, one, few, many)
    )
}

pub fn counts_line(counts: &Counts) -> String {
    format!(
        "{}, {}, {}",
        plural(counts.clients, "клиент", "клиента", "клиентов"),
        plural(counts.subscriptions, "подписка", "подписки", "подписок"),
        plural(counts.payments, "оплата", "оплаты", "оплат")
    )
}

pub fn size_text(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB * KB {
        format!("{:.0} КБ", (b / KB).max(1.0))
    } else if b < KB * KB * KB {
        format!("{:.1} МБ", b / KB / KB).replace('.', ",")
    } else {
        format!("{:.2} ГБ", b / KB / KB / KB).replace('.', ",")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunRow {
    id: i64,
    kind: String,
    state: String,
    file: Option<String>,
    size_bytes: Option<i64>,
    details: Value,
    error: Option<String>,
    admin_id: Option<i64>,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for RunRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(RunRow {
            id: row.try_get("id")?,
            kind: row.try_get("kind")?,
            state: row.try_get("state")?,
            file: row.try_get("file")?,
            size_bytes: row.try_get("size_bytes")?,
            details: row.try_get("details")?,
            error: row.try_get("error")?,
            admin_id: row.try_get("admin_id")?,
            started_at: row.try_get("started_at")?,
            finished_at: row.try_get("finished_at")?,
        })
    }
}

fn field<T: serde::de::DeserializeOwned>(details: Option<&Value>, key: &str) -> Option<T> {
    details
        .and_then(|d| d.get(key))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

const RUN_COLUMNS: &str =
    "id, kind, state, file, size_bytes, details, error, admin_id, started_at, finished_at";

#[derive(Debug)]
pub struct Made {
    pub run_id: i64,
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub manifest: Manifest,
}

pub struct BackupOps<'a> {
    services: &'a Services,
}

fn no_dir_problem(container: bool) -> &'static str {
    if container {
        "Папка для копий на сервере не подключена. Выполните на сервере: sudo stealthnet update"
    } else {
        "Папка для копий не задана. Укажите в переменной окружения SN_BACKUP_DIR папку, куда панель может записывать, и перезапустите панель."
    }
}

fn unwritable_problem(container: bool, dir: &std::path::Path) -> String {
    if container {
        "Панель не может записывать в папку для копий. Выполните на сервере: sudo stealthnet update"
            .into()
    } else {
        format!(
            "Панель не может записывать в папку для копий {}. Дайте пользователю, от которого работает панель, право на запись в неё или укажите в SN_BACKUP_DIR другую папку и перезапустите панель.",
            dir.display()
        )
    }
}

impl<'a> BackupOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        BackupOps { services }
    }

    fn runtime(&self) -> &Runtime {
        self.services.backups_runtime()
    }

    pub fn env(&self) -> BackupEnv {
        self.services
            .backup_env()
            .map(|env| (*env).clone())
            .unwrap_or_default()
    }

    fn dir(&self) -> ServiceResult<PathBuf> {
        let env = self.env();
        env.dir
            .ok_or_else(|| rejected(NO_DIR, no_dir_problem(env.container)))
    }

    pub async fn config(&self) -> ServiceResult<BackupConfig> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn
            .settings()
            .get_as::<BackupConfig>(CONFIG_KEY)
            .await?
            .unwrap_or_default())
    }

    async fn save_config(&self, config: &BackupConfig, by: Option<AdminId>) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.settings().set_as(CONFIG_KEY, config, by).await?;
        drop(conn);
        self.runtime().wake();
        Ok(())
    }

    async fn secret(&self, key: &str) -> ServiceResult<Option<String>> {
        let Some(secrets) = self.services.secrets() else {
            return Ok(None);
        };
        let mut conn = self.services.store().conn().await?;
        let Some(sealed) = conn.secrets().get(key).await? else {
            return Ok(None);
        };
        match secrets.open_string(key, &sealed) {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                tracing::warn!(key, error = %err, "секрет резервных копий не расшифрован");
                Ok(None)
            }
        }
    }

    async fn put_secret(&self, key: &str, value: Option<&str>, by: AdminId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        match value {
            Some(value) => {
                let secrets = self.services.secrets().ok_or(ServiceError::NoSecretKey)?;
                let sealed = secrets.seal_str(key, value)?;
                conn.secrets().set(key, &sealed, Some(by)).await?;
            }
            None => {
                conn.secrets().delete(key).await?;
            }
        }
        Ok(())
    }

    async fn audit(&self, entry: AuditEntry) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.audit().record(&entry).await?;
        Ok(())
    }

    fn app(&self) -> AppInfo {
        match self.services.update_config() {
            Some(config) => AppInfo {
                version: config.build.version.clone(),
                label: config.build.label.clone(),
                commit: config.build.commit.clone(),
            },
            None => AppInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                label: None,
                commit: None,
            },
        }
    }

    fn current_key(&self) -> Option<CurrentKey> {
        let raw = self.env().secret_key?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .ok()?;
        let sealed = SecretBox::from_key(&bytes).ok()?;
        Some(CurrentKey {
            check: crypt::key_check(&bytes),
            raw: bytes,
            sealed,
        })
    }

    pub async fn storage(&self) -> StorageView {
        let env = self.env();
        let Some(dir) = env.dir.clone() else {
            return StorageView {
                configured: false,
                writable: false,
                host_dir: env.host_dir,
                used_bytes: 0,
                free_bytes: None,
                problem: Some(no_dir_problem(env.container).into()),
            };
        };
        let probe = dir.clone();
        let container = env.container;
        let (writable, problem, used, free) = tokio::task::spawn_blocking(move || {
            if std::fs::create_dir_all(&probe).is_err() {
                return (
                    false,
                    Some("папка для копий не создаётся".to_string()),
                    0,
                    None,
                );
            }
            let test = probe.join(format!(".write-test-{}", Uuid::new_v4().simple()));
            let writable = std::fs::write(&test, b"ok").is_ok();
            let _ = std::fs::remove_file(&test);
            let used = list_files(&probe)
                .iter()
                .map(|(_, _, size)| *size)
                .sum::<u64>();
            let problem = (!writable).then(|| unwritable_problem(container, &probe));
            (writable, problem, used, free_space(&probe))
        })
        .await
        .unwrap_or((false, Some("папка для копий недоступна".into()), 0, None));
        StorageView {
            configured: true,
            writable,
            host_dir: env.host_dir,
            used_bytes: used,
            free_bytes: free,
            problem,
        }
    }

    async fn runs_for(&self, names: &[String]) -> ServiceResult<HashMap<String, RunRow>> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<RunRow> = sqlx::query_as(&format!(
            "SELECT DISTINCT ON (file) {RUN_COLUMNS} FROM backup_runs \
             WHERE file = ANY($1) AND kind <> 'restore' ORDER BY file, id DESC"
        ))
        .bind(names)
        .fetch_all(conn.conn())
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| row.file.clone().map(|file| (file, row)))
            .collect())
    }

    async fn emails(&self, ids: impl Iterator<Item = i64>) -> ServiceResult<HashMap<i64, String>> {
        let ids: Vec<i64> = ids.collect();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, email FROM admins WHERE id = ANY($1)")
                .bind(&ids)
                .fetch_all(conn.conn())
                .await?;
        Ok(rows.into_iter().collect())
    }

    fn run_view(row: RunRow, emails: &HashMap<i64, String>) -> Option<RunView> {
        Some(RunView {
            id: row.id,
            kind: RunKind::parse(&row.kind)?,
            state: row.state,
            file: row.file,
            error: row.error,
            started_at: row.started_at,
            finished_at: row.finished_at,
            by: row.admin_id.and_then(|id| emails.get(&id).cloned()),
            details: row.details,
        })
    }

    pub async fn copies(&self) -> ServiceResult<Vec<CopyView>> {
        let Some(dir) = self.env().dir else {
            return Ok(Vec::new());
        };
        let files = tokio::task::spawn_blocking(move || list_files(&dir))
            .await
            .map_err(internal)?;
        let names: Vec<String> = files.iter().map(|(name, _, _)| name.clone()).collect();
        let runs = self.runs_for(&names).await?;
        let emails = self
            .emails(runs.values().filter_map(|r| r.admin_id))
            .await?;
        Ok(files
            .into_iter()
            .take(LIST_MAX)
            .map(|(name, info, size)| {
                let run = runs.get(&name);
                let details = run.map(|r| &r.details);
                CopyView {
                    created_at: field(details, "created_at").unwrap_or(info.at),
                    kind: info.kind,
                    sealed: info.sealed,
                    key_included: field(details, "key_included"),
                    counts: field(details, "counts"),
                    app: field(details, "app"),
                    telegram: field(details, "telegram"),
                    s3: field(details, "s3"),
                    by: run
                        .and_then(|r| r.admin_id)
                        .and_then(|id| emails.get(&id).cloned()),
                    name,
                    size,
                }
            })
            .collect())
    }

    async fn last_run(&self) -> ServiceResult<Option<RunRow>> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_as(&format!(
            "SELECT {RUN_COLUMNS} FROM backup_runs WHERE kind <> 'restore' AND finished_at IS NOT NULL \
             ORDER BY id DESC LIMIT 1"
        ))
        .fetch_optional(conn.conn())
        .await?)
    }

    async fn recent_restores(&self) -> ServiceResult<Vec<RunRow>> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_as(&format!(
            "SELECT {RUN_COLUMNS} FROM backup_runs WHERE kind = 'restore' ORDER BY id DESC LIMIT 5"
        ))
        .fetch_all(conn.conn())
        .await?)
    }

    pub async fn view(&self, admin: Option<AdminId>) -> ServiceResult<BackupsView> {
        let config = self.config().await?;
        let password_set = self.has_secret(PASSWORD_KEY).await?;
        let s3_secret = self.has_secret(S3_SECRET_KEY).await?;
        let alerts = self.services.alerts().config().await?;
        let group = alerts.chat_id.map(|_| GroupView {
            title: alerts.title.clone(),
            forum: alerts.forum,
            topic: alerts.topic_for(AlertKind::Backups),
        });
        let last = self.last_run().await?;
        let restores = self.recent_restores().await?;
        let emails = self
            .emails(
                last.iter()
                    .chain(restores.iter())
                    .filter_map(|r| r.admin_id),
            )
            .await?;
        let next_at = match self.due(&config).await? {
            Due::Now(_) => Some(self.services.now()),
            Due::At(at) => Some(at),
            Due::Never => None,
        };
        let storage = self.storage().await;
        let next_at = if storage.configured { next_at } else { None };
        let totp_required = match admin {
            Some(admin) => {
                let mut conn = self.services.store().conn().await?;
                conn.admins().get(admin).await?.totp.is_some()
            }
            None => false,
        };
        Ok(BackupsView {
            config: ConfigView {
                schedule: config.schedule.clone(),
                keep: config.keep,
                telegram: config.telegram.clone(),
                s3: config.s3.as_ref().map(|s| S3View {
                    on: s.on,
                    endpoint: s.place.endpoint.clone(),
                    region: s.place.region.clone(),
                    bucket: s.place.bucket.clone(),
                    prefix: s.place.prefix.clone(),
                    access_key: s.place.access_key.clone(),
                    virtual_host: s.place.virtual_host,
                    secret_set: s3_secret,
                }),
                password_set,
                password_set_at: config.password_set_at.filter(|_| password_set),
            },
            storage,
            secret_key: self.services.secrets().is_some(),
            bot: self.services.bot_token().is_some(),
            group,
            copies: self.copies().await?,
            next_at,
            job: self.runtime().job(),
            last: last.and_then(|r| Self::run_view(r, &emails)),
            restores: restores
                .into_iter()
                .filter_map(|r| Self::run_view(r, &emails))
                .collect(),
            offset_minutes: self.services.settings().remna_utc_offset_minutes,
            remna_sync_path: REMNA_SYNC_PATH,
            totp_required,
        })
    }

    async fn has_secret(&self, key: &str) -> ServiceResult<bool> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.secrets().get(key).await?.is_some())
    }

    pub async fn update_config(
        &self,
        admin: AdminId,
        input: ConfigInput,
    ) -> ServiceResult<BackupsView> {
        if input.keep == 0 || input.keep > KEEP_MAX {
            return Err(rejected(
                "backups.keep",
                format!("Хранить можно от 1 до {KEEP_MAX} копий"),
            ));
        }
        let schedule = input
            .schedule
            .clean()
            .map_err(|e| rejected("backups.schedule", e))?;
        let mut config = self.config().await?;
        if config.schedule != schedule {
            config.schedule_since = Some(self.services.now());
        }
        let changed = serde_json::json!({
            "schedule": schedule,
            "keep": input.keep,
        });
        config.schedule = schedule;
        config.keep = input.keep;
        self.save_config(&config, Some(admin)).await?;
        self.audit(audit_entry(
            Actor::Admin(admin),
            "backups.settings_changed",
            changed,
            None,
        ))
        .await?;
        self.view(Some(admin)).await
    }

    async fn confirm(
        &self,
        admin: AdminId,
        password: &str,
        code: Option<&str>,
    ) -> ServiceResult<()> {
        self.services
            .auth()
            .confirm_admin(admin, password, code)
            .await
    }

    pub async fn set_password(
        &self,
        admin: AdminId,
        password: Option<&str>,
        confirm: (&str, Option<&str>),
        meta: &RequestMeta,
    ) -> ServiceResult<BackupsView> {
        self.confirm(admin, confirm.0, confirm.1).await?;
        let value = password.map(clean_password).transpose()?;
        if value.is_some() && self.services.secrets().is_none() {
            return Err(ServiceError::NoSecretKey);
        }
        self.put_secret(PASSWORD_KEY, value.as_deref(), admin)
            .await?;
        let mut config = self.config().await?;
        config.password_set_at = value.as_ref().map(|_| self.services.now());
        self.save_config(&config, Some(admin)).await?;
        self.audit(audit_entry(
            Actor::Admin(admin),
            if value.is_some() {
                "backups.password_set"
            } else {
                "backups.password_removed"
            },
            serde_json::json!({}),
            Some(meta),
        ))
        .await?;
        self.view(Some(admin)).await
    }

    pub async fn set_telegram(
        &self,
        admin: AdminId,
        target: TelegramTarget,
        confirm: (&str, Option<&str>),
        meta: &RequestMeta,
    ) -> ServiceResult<BackupsView> {
        let mut config = self.config().await?;
        if config.telegram == target {
            return self.view(Some(admin)).await;
        }
        if target != TelegramTarget::Off {
            self.confirm(admin, confirm.0, confirm.1).await?;
        }
        if let TelegramTarget::Chat { chat_id } = target
            && chat_id == 0
        {
            return Err(rejected("backups.chat", "Укажите номер чата"));
        }
        config.telegram = target.clone();
        self.save_config(&config, Some(admin)).await?;
        self.audit(audit_entry(
            Actor::Admin(admin),
            "backups.telegram_changed",
            serde_json::json!({ "telegram": target }),
            Some(meta),
        ))
        .await?;
        self.view(Some(admin)).await
    }

    pub async fn set_s3(
        &self,
        admin: AdminId,
        input: Option<S3Input>,
        confirm: (&str, Option<&str>),
        meta: &RequestMeta,
    ) -> ServiceResult<BackupsView> {
        let mut config = self.config().await?;
        let Some(input) = input else {
            if let Some(s3) = config.s3.as_mut() {
                s3.on = false;
            }
            self.save_config(&config, Some(admin)).await?;
            self.audit(audit_entry(
                Actor::Admin(admin),
                "backups.s3_changed",
                serde_json::json!({ "on": false }),
                Some(meta),
            ))
            .await?;
            return self.view(Some(admin)).await;
        };
        self.confirm(admin, confirm.0, confirm.1).await?;
        let place = S3Place {
            endpoint: input.endpoint,
            region: input.region,
            bucket: input.bucket,
            prefix: input.prefix,
            access_key: input.access_key,
            virtual_host: input.virtual_host,
        }
        .clean()
        .map_err(|e| rejected("backups.s3", e))?;
        let secret = input
            .secret_key
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let old = config.s3.as_ref().map(|s| s.place.clone());
        let moved = old.as_ref().is_none_or(|old| {
            old.endpoint != place.endpoint
                || old.bucket != place.bucket
                || old.access_key != place.access_key
        });
        if secret.is_none() && (moved || !self.has_secret(S3_SECRET_KEY).await?) {
            return Err(rejected(
                "backups.s3_secret",
                "Введите секретный ключ: при смене адреса, бакета или ключа доступа его нужно ввести заново",
            ));
        }
        if let Some(secret) = secret.as_deref() {
            self.put_secret(S3_SECRET_KEY, Some(secret), admin).await?;
        }
        config.s3 = Some(S3Settings {
            on: input.on,
            place: place.clone(),
        });
        self.save_config(&config, Some(admin)).await?;
        self.audit(audit_entry(
            Actor::Admin(admin),
            "backups.s3_changed",
            serde_json::json!({
                "on": input.on,
                "endpoint": place.endpoint,
                "bucket": place.bucket,
                "prefix": place.prefix,
            }),
            Some(meta),
        ))
        .await?;
        self.view(Some(admin)).await
    }

    async fn s3_client(&self, place: S3Place) -> ServiceResult<s3::Client> {
        let secret = self
            .secret(S3_SECRET_KEY)
            .await?
            .ok_or_else(|| rejected("backups.s3_secret", "Не задан секретный ключ хранилища"))?;
        s3::Client::new(place, secret).map_err(internal)
    }

    pub async fn check_s3(&self, input: Option<S3Input>) -> ServiceResult<String> {
        let (place, secret) = match input {
            Some(input) => {
                let place = S3Place {
                    endpoint: input.endpoint,
                    region: input.region,
                    bucket: input.bucket,
                    prefix: input.prefix,
                    access_key: input.access_key,
                    virtual_host: input.virtual_host,
                }
                .clean()
                .map_err(|e| rejected("backups.s3", e))?;
                let secret = match input
                    .secret_key
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                {
                    Some(secret) => secret,
                    None => {
                        let saved = self.config().await?.s3.map(|s| s.place);
                        let same = saved.as_ref().is_some_and(|old| {
                            old.endpoint == place.endpoint
                                && old.bucket == place.bucket
                                && old.access_key == place.access_key
                        });
                        if !same {
                            return Err(rejected(
                                "backups.s3_secret",
                                "Введите секретный ключ, чтобы проверить новое хранилище",
                            ));
                        }
                        self.secret(S3_SECRET_KEY).await?.ok_or_else(|| {
                            rejected("backups.s3_secret", "Введите секретный ключ")
                        })?
                    }
                };
                (place, secret)
            }
            None => {
                let place = self
                    .config()
                    .await?
                    .s3
                    .map(|s| s.place)
                    .ok_or_else(|| rejected("backups.s3", "Хранилище не настроено"))?;
                let secret = self
                    .secret(S3_SECRET_KEY)
                    .await?
                    .ok_or_else(|| rejected("backups.s3_secret", "Введите секретный ключ"))?;
                (place, secret)
            }
        };
        let client = s3::Client::new(place.clone(), secret).map_err(internal)?;
        let key = place.key_for(&format!(
            ".stealthnet-check-{}.txt",
            Uuid::new_v4().simple()
        ));
        client
            .put(&key, b"STEALTHNET backup check".to_vec())
            .await
            .map_err(|e| rejected("backups.s3_failed", e.readable()))?;
        Ok(match client.delete(&key).await {
            Ok(()) => format!(
                "Подключение работает: проверочный файл записан в {} и удалён",
                place.bucket
            ),
            Err(_) => format!(
                "Запись в {} работает. Проверочный файл {key} удалить не вышло: у ключа нет права удалять, это не мешает копиям",
                place.bucket
            ),
        })
    }

    async fn telegram_place(
        &self,
        target: &TelegramTarget,
    ) -> ServiceResult<(i64, Option<String>)> {
        match target {
            TelegramTarget::Off => Err(rejected(
                "backups.telegram_off",
                "Отправка в Telegram выключена",
            )),
            TelegramTarget::Chat { chat_id } => Ok((*chat_id, None)),
            TelegramTarget::Alerts => {
                let alerts = self.services.alerts().config().await?;
                let chat = alerts.chat_id.ok_or_else(|| {
                    rejected(
                        "backups.no_group",
                        "Группа уведомлений не подключена: подключите её в разделе «Уведомления в Telegram»",
                    )
                })?;
                Ok((chat, alerts.topic_for(AlertKind::Backups)))
            }
        }
    }

    fn bot(&self) -> ServiceResult<telegram::Bot> {
        let token = self.services.bot_token().ok_or_else(|| {
            rejected(
                "backups.no_bot",
                "Бот не подключён: подключите его в разделе «Настройки → Бот»",
            )
        })?;
        let proxy = self
            .services
            .egress()
            .proxy_for(sn_egress::Route::Telegram, None);
        telegram::Bot::new(&self.env().telegram_api, &token, proxy.as_deref()).map_err(internal)
    }

    async fn thread(
        &self,
        bot: &telegram::Bot,
        chat: i64,
        topic: Option<&str>,
    ) -> ServiceResult<Option<i64>> {
        let Some(name) = topic else {
            return Ok(None);
        };
        let alerts = self.services.alerts();
        if let Some(id) = alerts.topics().await?.get(name) {
            return Ok(Some(*id));
        }
        match bot.create_topic(chat, name).await {
            Ok(id) => {
                alerts.remember_topic(name, id).await?;
                Ok(Some(id))
            }
            Err(err) => {
                tracing::warn!(error = ?err, "тема для копий не создана, пишем в общий чат");
                if let telegram::TgError::Api { description, .. } = &err {
                    alerts.topics_refused(description).await?;
                }
                Ok(None)
            }
        }
    }

    pub async fn check_telegram(&self) -> ServiceResult<String> {
        let config = self.config().await?;
        let (chat, topic) = self.telegram_place(&config.telegram).await?;
        let bot = self.bot()?;
        let thread = self.thread(&bot, chat, topic.as_deref()).await?;
        let brand = self.services.settings().brand_name.clone();
        let text = format!(
            "<b>Проверка отправки копий</b>\nСюда будут приходить резервные копии {}.",
            escape_html(&brand)
        );
        let sent = bot.send_message(chat, thread, &text).await;
        match sent {
            Ok(()) => Ok("Проверочное сообщение отправлено".into()),
            Err(err) if thread.is_some() && err.thread_missing() => {
                if let Some(name) = topic.as_deref() {
                    self.services.alerts().forget_topic(name).await?;
                }
                let thread = self.thread(&bot, chat, topic.as_deref()).await?;
                bot.send_message(chat, thread, &text)
                    .await
                    .map_err(|e| rejected("backups.telegram_failed", e.readable()))?;
                Ok("Проверочное сообщение отправлено".into())
            }
            Err(err) => Err(rejected("backups.telegram_failed", err.readable())),
        }
    }

    pub async fn due(&self, config: &BackupConfig) -> ServiceResult<Due> {
        let mut conn = self.services.store().conn().await?;
        let last: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT max(slot) FROM backup_runs")
            .fetch_one(conn.conn())
            .await?;
        drop(conn);
        Ok(schedule::due(
            &config.schedule,
            config.schedule_since,
            last,
            self.services.now(),
            self.services.settings().remna_offset(),
        ))
    }

    pub async fn next_run_in(&self) -> ServiceResult<Option<std::time::Duration>> {
        if self.env().dir.is_none() {
            return Ok(None);
        }
        let config = self.config().await?;
        Ok(match self.due(&config).await? {
            Due::Now(_) => Some(std::time::Duration::ZERO),
            Due::At(at) => Some((at - self.services.now()).to_std().unwrap_or_default()),
            Due::Never => None,
        })
    }

    pub async fn run_scheduled(&self) -> ServiceResult<Option<Made>> {
        if self.env().dir.is_none() {
            return Ok(None);
        }
        let config = self.config().await?;
        let Due::Now(slot) = self.due(&config).await? else {
            return Ok(None);
        };
        let Ok(_guard) = self.runtime().lock.try_lock() else {
            return Ok(None);
        };
        self.make(RunKind::Auto, None, Some(slot), true)
            .await
            .map(Some)
    }

    pub async fn mark_stale(&self) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "UPDATE backup_runs SET state = 'failed', error = 'Копия прервалась: панель перезапустилась', \
             finished_at = now() WHERE state = 'running' AND started_at < now() - make_interval(hours => $1)",
        )
        .bind(STALE_RUN_HOURS as i32)
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    pub async fn start_manual(&self, admin: AdminId) -> ServiceResult<String> {
        self.dir()?;
        let guard = self
            .runtime()
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| rejected(BUSY, "Копия или восстановление уже идёт"))?;
        let services = self.services.clone();
        let id = Uuid::new_v4().to_string();
        tokio::spawn(async move {
            let _guard = guard;
            if let Err(err) = services
                .backups()
                .make(RunKind::Manual, Some(admin), None, true)
                .await
            {
                tracing::warn!(error = %err, "копия вручную не сделана");
            }
        });
        Ok(id)
    }

    pub async fn make_now(&self, kind: RunKind, admin: Option<AdminId>) -> ServiceResult<Made> {
        let _guard = self
            .runtime()
            .lock
            .try_lock()
            .map_err(|_| rejected(BUSY, "Копия или восстановление уже идёт"))?;
        self.make(kind, admin, None, kind != RunKind::BeforeRestore)
            .await
    }

    async fn start_run(
        &self,
        kind: RunKind,
        admin: Option<AdminId>,
        slot: Option<DateTime<Utc>>,
    ) -> ServiceResult<Option<i64>> {
        let mut conn = self.services.store().conn().await?;
        Ok(sqlx::query_scalar(
            "INSERT INTO backup_runs (kind, state, slot, admin_id) VALUES ($1, 'running', $2, $3) \
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(kind.as_str())
        .bind(slot)
        .bind(admin.map(|a| a.0))
        .fetch_optional(conn.conn())
        .await?)
    }

    async fn fail_run(&self, id: i64, error: &str) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "UPDATE backup_runs SET state = 'failed', error = $2, finished_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(error)
        .execute(conn.conn())
        .await?;
        Ok(())
    }

    async fn remna_users(&self) -> (Option<HashMap<i64, Value>>, RemnaInfo) {
        let now = self.services.now();
        let mut info = RemnaInfo {
            source: "panel".into(),
            taken_at: Some(now),
            ..RemnaInfo::default()
        };
        let api = match self.services.remna() {
            Ok(api) => api,
            Err(_) => {
                info.error = Some("Remnawave не подключена: снимок собран по данным панели".into());
                return (None, info);
            }
        };
        let mut users = HashMap::new();
        let mut cursor = None;
        loop {
            let page = api
                .stream_users(&sn_remnawave::UsersStreamQuery {
                    cursor: cursor.clone(),
                    size: Some(REMNA_PAGE),
                    ..Default::default()
                })
                .await;
            match page {
                Ok(page) => {
                    for user in page.users {
                        let id = user.id.0;
                        if let Ok(value) = serde_json::to_value(&user) {
                            users.insert(id, value);
                        }
                    }
                    if !page.has_more || page.next_cursor.is_none() || page.next_cursor == cursor {
                        break;
                    }
                    cursor = page.next_cursor;
                }
                Err(err) => {
                    info.error = Some(format!(
                        "Remnawave не ответила ({err}): снимок собран по данным панели"
                    ));
                    return (None, info);
                }
            }
        }
        info.source = "remnawave".into();
        info.users = users.len() as u64;
        (Some(users), info)
    }

    async fn remna_lines(
        conn: &mut PgConnection,
        schema: &dump::Schema,
        users: Option<&HashMap<i64, Value>>,
        info: &mut RemnaInfo,
    ) -> ServiceResult<Vec<u8>> {
        let linked = schema.table("subscriptions").is_some_and(|t| {
            t.columns.iter().any(|c| c == "remna_user_id")
                && t.columns.iter().any(|c| c == "client_id")
        });
        let mut lines: Vec<Value> = Vec::new();
        if linked {
            let rows: Vec<(i64, i64, i64, Value)> = sqlx::query_as(&format!(
                "SELECT s.id, s.client_id, s.remna_user_id, to_jsonb(s) FROM {} s \
                 WHERE s.remna_user_id IS NOT NULL ORDER BY s.id",
                dump::qualified(&schema.name, "subscriptions")
            ))
            .fetch_all(&mut *conn)
            .await?;
            for (id, client_id, remna_user_id, row) in rows {
                let user = users.and_then(|u| u.get(&remna_user_id)).cloned();
                if users.is_some() && user.is_none() {
                    info.missing += 1;
                }
                let panel: serde_json::Map<String, Value> = [
                    "tariff_id",
                    "kind",
                    "remna_username",
                    "short_uuid",
                    "status",
                    "expire_at",
                    "traffic_limit_bytes",
                    "traffic_strategy",
                    "hwid_limit",
                    "extra_devices",
                    "ended_at",
                ]
                .iter()
                .filter_map(|key| row.get(*key).map(|v| (key.to_string(), v.clone())))
                .collect();
                lines.push(serde_json::json!({
                    "subscription_id": id,
                    "client_id": client_id,
                    "remna_user_id": remna_user_id,
                    "user": user,
                    "panel": panel,
                }));
            }
        }
        info.linked = lines.len() as u64;
        let mut out = serde_json::to_vec(&serde_json::json!({
            "snapshot": "remnawave",
            "taken_at": info.taken_at,
            "source": info.source,
            "users": info.users,
            "linked": info.linked,
            "missing": info.missing,
            "error": info.error,
        }))
        .map_err(internal)?;
        out.push(b'\n');
        for line in lines {
            serde_json::to_writer(&mut out, &line).map_err(internal)?;
            out.push(b'\n');
        }
        Ok(out)
    }

    fn update_job(&self, stage: &'static str, detail: Option<String>) {
        self.runtime().update(|job| {
            job.stage = stage;
            job.detail = detail;
        });
    }

    async fn make(
        &self,
        kind: RunKind,
        admin: Option<AdminId>,
        slot: Option<DateTime<Utc>>,
        deliver: bool,
    ) -> ServiceResult<Made> {
        let own_job = self.runtime().job().is_none_or(|j| j.state != "running");
        if own_job {
            self.runtime().set_job(Some(Job {
                id: Uuid::new_v4().to_string(),
                kind: "backup",
                state: "running",
                stage: "dump",
                detail: None,
                started_at: self.services.now(),
                finished_at: None,
                error: None,
                result: None,
                watch: None,
            }));
        }
        let Some(run_id) = self.start_run(kind, admin, slot).await? else {
            if own_job {
                self.runtime().set_job(None);
            }
            return Err(rejected(BUSY, "Эта копия по расписанию уже сделана"));
        };
        let result = self.make_inner(run_id, kind, deliver).await;
        match &result {
            Ok(made) => {
                if own_job {
                    self.runtime().update(|job| {
                        job.state = "done";
                        job.stage = "done";
                        job.finished_at = Some(self.services.now());
                        job.result =
                            Some(serde_json::json!({ "file": made.name, "size": made.size }));
                    });
                }
            }
            Err(err) => {
                let text = err.to_string();
                let _ = self.fail_run(run_id, &text).await;
                if own_job {
                    self.runtime().update(|job| {
                        job.state = "failed";
                        job.finished_at = Some(self.services.now());
                        job.error = Some(text);
                    });
                }
            }
        }
        result
    }

    async fn make_inner(&self, run_id: i64, kind: RunKind, deliver: bool) -> ServiceResult<Made> {
        let env = self.env();
        let dir = self.dir()?;
        let probe = dir.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&probe))
            .await
            .map_err(internal)?
            .map_err(|e| rejected(NO_DIR, format!("Папка для копий не создаётся: {e}")))?;
        let password = self.secret(PASSWORD_KEY).await?;
        let key_raw = env.secret_key.as_deref().and_then(|raw| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(raw.trim())
                .ok()
        });
        let include_key = password.is_some() && key_raw.is_some();
        self.update_job("remnawave", None);
        let (users, mut remna) = self.remna_users().await;
        self.update_job("dump", None);
        let mut conn = self
            .services
            .store()
            .pool()
            .acquire()
            .await
            .map_err(sn_store::StoreError::from)?
            .detach();
        let size: i64 = sqlx::query_scalar(
            "SELECT coalesce(sum(pg_total_relation_size(c.oid)), 0)::bigint FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p', 'm')",
        )
        .fetch_one(&mut conn)
        .await?;
        if let Some(free) = free_space(&dir)
            && free < (size.max(0) as u64) / 2 + 64 * 1024 * 1024
        {
            return Err(rejected(
                "backups.no_space",
                format!(
                    "На диске сервера мало места для копии: свободно {}, база занимает {}",
                    size_text(free),
                    size_text(size.max(0) as u64)
                ),
            ));
        }
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_lock($1, hashtext(current_schema()))")
                .bind(LOCK_CLASS)
                .fetch_one(&mut conn)
                .await?;
        if !locked {
            return Err(rejected(BUSY, "Копию уже делает другой процесс панели"));
        }
        sqlx::Executor::execute(&mut conn, "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await?;
        let schema_name: String = sqlx::query_scalar("SELECT current_schema()::text")
            .fetch_one(&mut conn)
            .await?;
        let schema = dump::inspect(&mut conn, &schema_name, &dump::SKIPPED).await?;
        let migrations: Vec<i64> = sqlx::query_scalar(&format!(
            "SELECT version FROM {} WHERE success ORDER BY version",
            dump::qualified(&schema_name, "_sqlx_migrations")
        ))
        .fetch_all(&mut conn)
        .await?;
        let tables = dump::table_entries(&mut conn, &schema).await?;
        let sequences = dump::sequence_entries(&mut conn, &schema).await?;
        let rows_of = |name: &str| {
            tables
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.rows)
                .unwrap_or(0)
        };
        let active: Option<u64> = if schema
            .table("subscriptions")
            .is_some_and(|t| t.columns.iter().any(|c| c == "ended_at"))
        {
            let n: i64 = sqlx::query_scalar(&format!(
                "SELECT count(*) FROM {} WHERE ended_at IS NULL",
                dump::qualified(&schema_name, "subscriptions")
            ))
            .fetch_one(&mut conn)
            .await?;
            Some(n.max(0) as u64)
        } else {
            None
        };
        let postgres: Option<String> = sqlx::query_scalar("SHOW server_version")
            .fetch_one(&mut conn)
            .await
            .ok()
            .and_then(|v: String| v.split_whitespace().next().map(str::to_string));
        let remna_raw = Self::remna_lines(&mut conn, &schema, users.as_ref(), &mut remna).await?;
        let created_at = self.services.now();
        let settings = self.services.settings();
        let manifest = Manifest {
            format: format::FORMAT.into(),
            format_version: format::FORMAT_VERSION,
            created_at,
            kind,
            app: self.app(),
            postgres,
            brand: Some(settings.brand_name.clone()),
            public_url: settings.public_url.clone(),
            migrations,
            counts: Counts {
                clients: rows_of("clients"),
                subscriptions: rows_of("subscriptions"),
                active_subscriptions: active,
                payments: rows_of("payments"),
            },
            tables,
            sequences,
            secret_key: KeyInfo {
                present: key_raw.is_some(),
                included: include_key,
                check: key_raw.as_deref().map(crypt::key_check),
            },
            remnawave: Some(remna.clone()),
        };
        let mut n = 0;
        let name = loop {
            let name = format::numbered_name(created_at, kind, password.is_some(), n);
            if !dir.join(&name).exists()
                && !dir
                    .join(format!("{}{name}", format::PARTIAL_PREFIX))
                    .exists()
            {
                break name;
            }
            n += 1;
            if n > 50 {
                return Err(internal("не удалось подобрать имя файла копии"));
            }
        };
        let partial = dir.join(format!("{}{name}", format::PARTIAL_PREFIX));
        let target = dir.join(&name);
        let written = dump::write(
            &mut conn,
            &schema,
            &manifest,
            dump::Extras {
                remna: remna_raw,
                secret_key: if include_key {
                    env.secret_key.clone().map(|k| k.trim().as_bytes().to_vec())
                } else {
                    None
                },
            },
            dump::Output {
                path: partial.clone(),
                password: password.clone(),
                params: env.kdf,
                mtime: created_at.timestamp().max(0) as u64,
            },
        )
        .await;
        let _ = sqlx::Executor::execute(&mut conn, "COMMIT").await;
        let _ = sqlx::query("SELECT pg_advisory_unlock($1, hashtext(current_schema()))")
            .bind(LOCK_CLASS)
            .execute(&mut conn)
            .await;
        drop(conn);
        let written = match written {
            Ok(written) => written,
            Err(err) => {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(err);
            }
        };
        self.update_job("verify", None);
        let check_path = partial.clone();
        let check_password = password.clone();
        let inspected = tokio::task::spawn_blocking(move || {
            inspect_file(&check_path, check_password.as_deref())
        })
        .await
        .map_err(internal)?;
        let inspection = match inspected {
            Ok(inspection) if inspection.sha256 == written.sha256 => inspection,
            Ok(_) => {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(rejected(
                    "backups.verify",
                    "Копия не прошла проверку: файл изменился после записи",
                ));
            }
            Err(err) => {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err(rejected(
                    "backups.verify",
                    format!("Копия не прошла проверку после записи: {err}"),
                ));
            }
        };
        tokio::fs::rename(&partial, &target)
            .await
            .map_err(|e| ServiceError::Internal(format!("файл копии не переименован: {e}")))?;
        let details = serde_json::json!({
            "created_at": created_at,
            "counts": manifest.counts,
            "app": manifest.app.shown(),
            "key_included": include_key,
            "sealed": password.is_some(),
            "remnawave": remna,
            "tables": manifest.tables.len(),
            "rows": manifest.tables.iter().map(|t| t.rows).sum::<u64>(),
        });
        {
            let mut conn = self.services.store().conn().await?;
            sqlx::query(
                "UPDATE backup_runs SET state = 'done', file = $2, size_bytes = $3, sha256 = $4, \
                 encrypted = $5, with_key = $6, details = $7 WHERE id = $1",
            )
            .bind(run_id)
            .bind(&name)
            .bind(inspection.size as i64)
            .bind(&inspection.sha256)
            .bind(password.is_some())
            .bind(include_key)
            .bind(&details)
            .execute(conn.conn())
            .await?;
        }
        let config = self.config().await?;
        self.retention(&dir, config.keep, &name).await;
        let made = Made {
            run_id,
            name: name.clone(),
            path: target.clone(),
            size: inspection.size,
            manifest,
        };
        if deliver {
            self.deliver(run_id, &made, &config).await;
        }
        let mut conn = self.services.store().conn().await?;
        sqlx::query("UPDATE backup_runs SET finished_at = now() WHERE id = $1")
            .bind(run_id)
            .execute(conn.conn())
            .await?;
        Ok(made)
    }

    async fn retention(&self, dir: &Path, keep: u32, fresh: &str) {
        let dir = dir.to_path_buf();
        let fresh = fresh.to_string();
        let removed = tokio::task::spawn_blocking(move || {
            let mut removed = Vec::new();
            let files = list_files(&dir);
            for (name, _, _) in files.into_iter().skip(keep.max(1) as usize) {
                if name == fresh {
                    continue;
                }
                if std::fs::remove_file(dir.join(&name)).is_ok() {
                    removed.push(name);
                }
            }
            let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(24 * 3600);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with(format::PARTIAL_PREFIX)
                        && entry
                            .metadata()
                            .and_then(|m| m.modified())
                            .is_ok_and(|at| at < cutoff)
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
            removed
        })
        .await
        .unwrap_or_default();
        if !removed.is_empty() {
            tracing::info!(count = removed.len(), "старые резервные копии удалены");
        }
    }

    async fn save_delivery(&self, run_id: i64, key: &str, delivery: &Delivery) {
        let Ok(value) = serde_json::to_value(delivery) else {
            return;
        };
        let Ok(mut conn) = self.services.store().conn().await else {
            return;
        };
        let _ = sqlx::query(
            "UPDATE backup_runs SET details = jsonb_set(details, ARRAY[$2], $3, true) WHERE id = $1",
        )
        .bind(run_id)
        .bind(key)
        .bind(value)
        .execute(conn.conn())
        .await;
    }

    async fn deliver(&self, run_id: i64, made: &Made, config: &BackupConfig) {
        if config.telegram != TelegramTarget::Off {
            self.update_job("telegram", None);
            let delivery = match self.send_telegram(made, &config.telegram).await {
                Ok(parts) => Delivery {
                    state: "sent".into(),
                    parts: Some(parts),
                    at: Some(self.services.now()),
                    ..Delivery::default()
                },
                Err(err) => Delivery {
                    state: "failed".into(),
                    error: Some(err.to_string()),
                    at: Some(self.services.now()),
                    ..Delivery::default()
                },
            };
            self.save_delivery(run_id, "telegram", &delivery).await;
        }
        if let Some(s3) = config.s3.as_ref().filter(|s| s.on) {
            self.update_job("s3", None);
            let key = s3.place.key_for(&made.name);
            let result = match self.s3_client(s3.place.clone()).await {
                Ok(client) => client
                    .upload_file(&key, &made.path, made.size)
                    .await
                    .map_err(|e| e.readable()),
                Err(err) => Err(err.to_string()),
            };
            let delivery = match result {
                Ok(()) => Delivery {
                    state: "sent".into(),
                    key: Some(key),
                    at: Some(self.services.now()),
                    ..Delivery::default()
                },
                Err(error) => Delivery {
                    state: "failed".into(),
                    error: Some(error),
                    key: Some(key),
                    at: Some(self.services.now()),
                    ..Delivery::default()
                },
            };
            self.save_delivery(run_id, "s3", &delivery).await;
        }
    }

    fn caption(&self, made: &Made, part: u64, parts: u64) -> String {
        let manifest = &made.manifest;
        let offset = self.services.settings().remna_offset();
        let local = manifest.created_at.with_timezone(&offset);
        let brand = manifest.brand.clone().unwrap_or_default();
        let mut lines = vec![
            format!("<b>Резервная копия {}</b>", escape_html(&brand)),
            format!(
                "{} {} · {} · {}",
                local.format("%d.%m.%Y %H:%M"),
                utc_label(offset),
                kind_title(manifest.kind),
                size_text(made.size)
            ),
            counts_line(&manifest.counts),
        ];
        lines.push(if manifest.secret_key.included {
            "Зашифрована паролем копий, ключ шифрования секретов внутри.".into()
        } else if made.name.ends_with(format::SEALED_EXT) {
            "Зашифрована паролем копий.".into()
        } else {
            "Без пароля: ключей платёжек в копии нет.".into()
        });
        if parts > 1 {
            lines.push(format!(
                "Часть {} из {parts}. Соберите части в один файл: <code>cat {}.part* &gt; {}</code>",
                part + 1,
                escape_html(&made.name),
                escape_html(&made.name)
            ));
        }
        lines.join("\n")
    }

    async fn send_telegram(&self, made: &Made, target: &TelegramTarget) -> ServiceResult<u64> {
        let (chat, topic) = self.telegram_place(target).await?;
        let bot = self.bot()?;
        let mut thread = self.thread(&bot, chat, topic.as_deref()).await?;
        let part_size = self.env().telegram_part.max(1);
        let parts = telegram::parts(made.size, part_size);
        for part in 0..parts {
            let offset = part * part_size;
            let len = part_size.min(made.size.saturating_sub(offset));
            let path = made.path.clone();
            let bytes = tokio::task::spawn_blocking(move || -> io::Result<Vec<u8>> {
                use std::io::{Read, Seek, SeekFrom};
                let mut file = std::fs::File::open(&path)?;
                file.seek(SeekFrom::Start(offset))?;
                let mut out = Vec::with_capacity(len as usize);
                file.take(len).read_to_end(&mut out)?;
                Ok(out)
            })
            .await
            .map_err(internal)?
            .map_err(|e| ServiceError::Internal(format!("файл копии не читается: {e}")))?;
            let name = telegram::part_name(&made.name, part, parts);
            let caption = self.caption(made, part, parts);
            let mut tries = 0;
            loop {
                tries += 1;
                match bot
                    .send_document(chat, thread, &name, bytes.clone(), &caption)
                    .await
                {
                    Ok(()) => break,
                    Err(err)
                        if err.thread_missing() && thread.is_some() && tries < TELEGRAM_TRIES =>
                    {
                        if let Some(topic) = topic.as_deref() {
                            self.services.alerts().forget_topic(topic).await?;
                        }
                        thread = self.thread(&bot, chat, topic.as_deref()).await?;
                    }
                    Err(err) if err.retry_after().is_some() && tries < TELEGRAM_TRIES => {
                        let wait = err.retry_after().unwrap_or(5).min(60);
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    }
                    Err(telegram::TgError::Transport(_)) if tries < TELEGRAM_TRIES => {
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                    Err(err) => {
                        return Err(rejected("backups.telegram_failed", err.readable()));
                    }
                }
            }
        }
        Ok(parts)
    }

    pub async fn resend(&self, name: &str, admin: AdminId) -> ServiceResult<BackupsView> {
        let path = self.file_path(name)?;
        let _guard = self
            .runtime()
            .lock
            .try_lock()
            .map_err(|_| rejected(BUSY, "Копия или восстановление уже идёт"))?;
        let runs = self.runs_for(&[name.to_string()]).await?;
        let run_id = match runs.get(name) {
            Some(run) => run.id,
            None => self
                .start_run(RunKind::Manual, Some(admin), None)
                .await?
                .ok_or_else(|| internal("запись о копии не создана"))?,
        };
        let size = tokio::fs::metadata(&path).await.map_err(internal)?.len();
        let password = self.secret(PASSWORD_KEY).await?;
        let inspect_path = path.clone();
        let inspection =
            tokio::task::spawn_blocking(move || inspect_file(&inspect_path, password.as_deref()))
                .await
                .map_err(internal)?
                .map_err(load::archive_error)?;
        let made = Made {
            run_id,
            name: name.to_string(),
            path,
            size,
            manifest: inspection.manifest,
        };
        let config = self.config().await?;
        self.deliver(run_id, &made, &config).await;
        let mut conn = self.services.store().conn().await?;
        sqlx::query("UPDATE backup_runs SET finished_at = now(), state = 'done' WHERE id = $1")
            .bind(run_id)
            .execute(conn.conn())
            .await?;
        drop(conn);
        self.view(Some(admin)).await
    }

    pub fn file_path(&self, name: &str) -> ServiceResult<PathBuf> {
        if !format::safe_name(name) {
            return Err(ServiceError::NotFound("копия"));
        }
        let path = self.dir()?.join(name);
        if !path.is_file() {
            return Err(ServiceError::NotFound("копия"));
        }
        Ok(path)
    }

    pub async fn download_ticket(
        &self,
        name: &str,
        admin: AdminId,
        confirm: (&str, Option<&str>),
    ) -> ServiceResult<String> {
        let path = self.file_path(name)?;
        tokio::fs::metadata(&path)
            .await
            .map_err(|_| ServiceError::NotFound("копия"))?;
        self.confirm(admin, confirm.0, confirm.1).await?;
        let token = hex::encode(rand::random::<[u8; 16]>());
        let now = self.services.now();
        let mut tickets = self.runtime().tickets();
        tickets.retain(|t| t.expires_at > now);
        tickets.push(DownloadTicket {
            token: token.clone(),
            name: name.to_string(),
            admin,
            expires_at: now + chrono::Duration::seconds(DOWNLOAD_TICKET_SECONDS),
        });
        Ok(token)
    }

    fn take_ticket(&self, name: &str, admin: AdminId, token: &str) -> bool {
        let now = self.services.now();
        let mut tickets = self.runtime().tickets();
        tickets.retain(|t| t.expires_at > now);
        let found = tickets.iter().position(|t| {
            t.admin == admin
                && t.name == name
                && t.token.len() == token.len()
                && t.token
                    .bytes()
                    .zip(token.bytes())
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
        });
        match found {
            Some(at) => {
                tickets.remove(at);
                true
            }
            None => false,
        }
    }

    pub async fn download(
        &self,
        name: &str,
        admin: AdminId,
        ticket: &str,
        meta: &RequestMeta,
    ) -> ServiceResult<(PathBuf, u64)> {
        let path = self.file_path(name)?;
        if !self.take_ticket(name, admin, ticket) {
            return Err(rejected(
                DOWNLOAD_CONFIRM,
                "Скачивание копии подтверждается паролем: нажмите «Скачать» ещё раз",
            ));
        }
        let size = tokio::fs::metadata(&path).await.map_err(internal)?.len();
        self.audit(audit_entry(
            Actor::Admin(admin),
            "backups.downloaded",
            serde_json::json!({ "file": name, "size": size, "user_agent": meta.user_agent }),
            Some(meta),
        ))
        .await?;
        Ok((path, size))
    }

    pub async fn delete(
        &self,
        name: &str,
        admin: AdminId,
        meta: &RequestMeta,
    ) -> ServiceResult<BackupsView> {
        let path = self.file_path(name)?;
        let _guard = self
            .runtime()
            .lock
            .try_lock()
            .map_err(|_| rejected(BUSY, "Копия или восстановление уже идёт"))?;
        tokio::fs::remove_file(&path).await.map_err(internal)?;
        self.audit(audit_entry(
            Actor::Admin(admin),
            "backups.deleted",
            serde_json::json!({ "file": name }),
            Some(meta),
        ))
        .await?;
        drop(_guard);
        self.view(Some(admin)).await
    }

    pub fn upload_path(&self, id: Uuid) -> PathBuf {
        self.env()
            .upload_dir
            .join(format!("{}.upload", id.simple()))
    }

    pub async fn cleanup_uploads(&self) {
        let dir = self.env().upload_dir;
        let _ = tokio::task::spawn_blocking(move || {
            let cutoff = std::time::SystemTime::now()
                - std::time::Duration::from_secs((UPLOAD_TTL_HOURS * 3600) as u64);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .is_ok_and(|at| at < cutoff)
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        })
        .await;
    }

    pub async fn upload_view(&self, id: Uuid, name: &str) -> ServiceResult<UploadView> {
        let path = self.upload_path(id);
        let size = tokio::fs::metadata(&path)
            .await
            .map_err(|_| ServiceError::NotFound("загрузка"))?
            .len();
        let probe = path.clone();
        let kind = tokio::task::spawn_blocking(move || archive::detect(&probe))
            .await
            .map_err(internal)?;
        let kind = match kind {
            Ok(kind) => kind,
            Err(err) => {
                let _ = tokio::fs::remove_file(&path).await;
                return Err(load::archive_error(err));
            }
        };
        Ok(UploadView {
            id,
            size,
            sealed: kind == Kind::Sealed,
            name: name.to_string(),
        })
    }

    pub async fn discard_upload(&self, id: Uuid) -> ServiceResult<()> {
        let _ = tokio::fs::remove_file(self.upload_path(id)).await;
        Ok(())
    }

    fn source_path(&self, source: &Source) -> ServiceResult<(PathBuf, String)> {
        match source {
            Source::File { name } => Ok((self.file_path(name)?, name.clone())),
            Source::Upload { id } => {
                let path = self.upload_path(*id);
                if !path.is_file() {
                    return Err(rejected(
                        "backups.upload_gone",
                        "Загруженный файл не найден: загрузите его ещё раз",
                    ));
                }
                Ok((path, "загруженный файл".into()))
            }
        }
    }

    pub async fn preview(&self, source: &Source, password: Option<&str>) -> ServiceResult<Preview> {
        let (path, label) = self.source_path(source)?;
        let password = match password.filter(|p| !p.is_empty()) {
            Some(password) => Some(password.to_string()),
            None => match source {
                Source::File { .. } => self.secret(PASSWORD_KEY).await?,
                Source::Upload { .. } => None,
            },
        };
        let inspection =
            tokio::task::spawn_blocking(move || inspect_file(&path, password.as_deref()))
                .await
                .map_err(internal)?
                .map_err(load::archive_error)?;
        self.preview_of(&inspection, label)
    }

    pub fn preview_of(&self, inspection: &Inspection, label: String) -> ServiceResult<Preview> {
        let manifest = &inspection.manifest;
        load::check_manifest(manifest).map_err(|e| rejected("backups.restore", e))?;
        let current = self.app();
        let pending = load::pending_after(manifest);
        let current_key = self.current_key();
        let same_key = current_key
            .as_ref()
            .is_some_and(|k| manifest.secret_key.check.as_deref() == Some(k.check.as_str()));
        let secrets_in_copy = manifest
            .tables
            .iter()
            .find(|t| t.name == "secrets")
            .map(|t| t.rows)
            .unwrap_or(0);
        let keys = if same_key {
            "same"
        } else if current_key.is_none() {
            "no_server_key"
        } else if inspection.key_in_file {
            "from_copy"
        } else if secrets_in_copy == 0 && !manifest.secret_key.present {
            "none"
        } else {
            "lost"
        };
        let mut warnings = Vec::new();
        if keys == "lost" {
            warnings.push(
                "Копия сделана без пароля на сервере с другим ключом шифрования: ключи платёжек, токен бота и другие секреты придётся ввести заново, двухфакторный вход администраторов и клиентов выключится, сохранённые карты для автопродления отвяжутся".into(),
            );
        }
        if keys == "no_server_key" {
            warnings.push(
                "На этом сервере не задан ключ шифрования SN_SECRET_KEY: секреты из копии сохранить не получится".into(),
            );
        }
        if let Some(remna) = manifest.remnawave.as_ref().and_then(|r| r.error.clone()) {
            warnings.push(remna);
        }
        Ok(Preview {
            source: label,
            size: inspection.size,
            sealed: inspection.kind == Kind::Sealed,
            created_at: manifest.created_at,
            kind: manifest.kind,
            app: manifest.app.shown(),
            current_app: current.shown(),
            same_version: pending.is_empty(),
            migrations_to_apply: pending.len(),
            counts: manifest.counts,
            tables: manifest.tables.len(),
            rows: manifest.tables.iter().map(|t| t.rows).sum(),
            remnawave: manifest.remnawave.clone(),
            brand: manifest.brand.clone(),
            public_url: manifest.public_url.clone(),
            keys,
            secrets_in_copy,
            warnings,
        })
    }

    pub async fn start_restore(
        &self,
        source: Source,
        password: Option<String>,
        admin: AdminId,
        confirm: (&str, Option<&str>),
        meta: &RequestMeta,
    ) -> ServiceResult<String> {
        self.confirm(admin, confirm.0, confirm.1).await?;
        let guard = self
            .runtime()
            .lock
            .clone()
            .try_lock_owned()
            .map_err(|_| rejected(BUSY, "Копия или восстановление уже идёт"))?;
        let (path, label) = self.source_path(&source)?;
        let password = match password.filter(|p| !p.is_empty()) {
            Some(password) => Some(password),
            None => match source {
                Source::File { .. } => self.secret(PASSWORD_KEY).await?,
                Source::Upload { .. } => None,
            },
        };
        let watch = hex::encode(rand::random::<[u8; 16]>());
        self.runtime().set_job(Some(Job {
            id: Uuid::new_v4().to_string(),
            kind: "restore",
            state: "running",
            stage: "check",
            detail: None,
            started_at: self.services.now(),
            finished_at: None,
            error: None,
            result: None,
            watch: Some(watch.clone()),
        }));
        self.audit(audit_entry(
            Actor::Admin(admin),
            "backups.restore_started",
            serde_json::json!({ "source": label, "user_agent": meta.user_agent }),
            Some(meta),
        ))
        .await?;
        let services = self.services.clone();
        let upload = matches!(source, Source::Upload { .. }).then(|| path.clone());
        tokio::spawn(async move {
            let _guard = guard;
            let ops = services.backups();
            let result = ops
                .restore_inner(&path, &label, password, Some(admin))
                .await;
            if let Some(upload) = upload
                && result.is_ok()
            {
                let _ = tokio::fs::remove_file(upload).await;
            }
            if let Err(err) = result {
                tracing::warn!(error = %err, "восстановление из копии не прошло");
            }
        });
        Ok(watch)
    }

    pub fn status(&self, watch: &str) -> Option<Job> {
        let job = self.runtime().job()?;
        let token = job.watch.as_deref()?;
        let same = token.len() == watch.len()
            && token
                .bytes()
                .zip(watch.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        same.then_some(job)
    }

    pub async fn restore_now(&self, path: &Path, password: Option<String>) -> ServiceResult<Value> {
        let _guard = self
            .runtime()
            .lock
            .try_lock()
            .map_err(|_| rejected(BUSY, "Копия или восстановление уже идёт"))?;
        self.runtime().set_job(Some(Job {
            id: Uuid::new_v4().to_string(),
            kind: "restore",
            state: "running",
            stage: "check",
            detail: None,
            started_at: self.services.now(),
            finished_at: None,
            error: None,
            result: None,
            watch: None,
        }));
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "файл".into());
        self.restore_inner(path, &label, password, None).await
    }

    async fn restore_inner(
        &self,
        path: &Path,
        label: &str,
        password: Option<String>,
        admin: Option<AdminId>,
    ) -> ServiceResult<Value> {
        let result = self.restore_steps(path, label, password, admin).await;
        let now = self.services.now();
        match &result {
            Ok(value) => {
                let value = value.clone();
                self.runtime().update(|job| {
                    job.state = "done";
                    job.stage = "done";
                    job.finished_at = Some(now);
                    job.result = Some(value);
                });
            }
            Err(err) => {
                let text = err.to_string();
                self.runtime().update(|job| {
                    job.state = "failed";
                    job.finished_at = Some(now);
                    job.error = Some(text.clone());
                });
                let mut conn = self.services.store().conn().await?;
                sqlx::query(
                    "INSERT INTO backup_runs (kind, state, file, error, admin_id, details, finished_at) \
                     VALUES ('restore', 'failed', $1, $2, $3, $4, now())",
                )
                .bind(label)
                .bind(&text)
                .bind(admin.map(|a| a.0))
                .bind(serde_json::json!({ "source": label }))
                .execute(conn.conn())
                .await?;
            }
        }
        result
    }

    async fn restore_steps(
        &self,
        path: &Path,
        label: &str,
        password: Option<String>,
        admin: Option<AdminId>,
    ) -> ServiceResult<Value> {
        self.update_job("check", None);
        let check_path = path.to_path_buf();
        let check_password = password.clone();
        let inspection = tokio::task::spawn_blocking(move || {
            inspect_file(&check_path, check_password.as_deref())
        })
        .await
        .map_err(internal)?
        .map_err(load::archive_error)?;
        let preview = self.preview_of(&inspection, label.to_string())?;
        self.update_job("pause", None);
        let paused = self.services.pause_gate().pause(DRAIN).await;
        self.update_job("before", None);
        let before = if self.env().dir.is_some() {
            match self.make(RunKind::BeforeRestore, admin, None, false).await {
                Ok(made) => Some(made.name),
                Err(err) => {
                    drop(paused);
                    return Err(rejected(
                        "backups.before_failed",
                        format!(
                            "Копия текущей базы не сохранилась, восстановление не начато: {err}"
                        ),
                    ));
                }
            }
        } else {
            None
        };
        self.update_job("load", None);
        let mut conn = self
            .services
            .store()
            .pool()
            .acquire()
            .await
            .map_err(sn_store::StoreError::from)?
            .detach();
        sqlx::Executor::execute(&mut conn, "BEGIN").await?;
        let runtime = self.runtime();
        let progress = |stage: Stage| {
            let (name, detail) = match stage {
                Stage::Load { table, tables } => {
                    ("load", Some(format!("таблица {table} из {tables}")))
                }
                Stage::Migrate => ("migrate", None),
                Stage::Apply => ("apply", None),
                Stage::Keys => ("keys", None),
            };
            runtime.update(|job| {
                job.stage = name;
                job.detail = detail;
            });
        };
        let loaded = load::load(
            &mut conn,
            LoadPlan {
                path: path.to_path_buf(),
                password: password.clone(),
                current_key: self.current_key(),
            },
            &progress,
        )
        .await;
        let (manifest, report) = match loaded {
            Ok(done) => done,
            Err(err) => {
                let _ = sqlx::Executor::execute(&mut conn, "ROLLBACK").await;
                drop(conn);
                drop(paused);
                return Err(err);
            }
        };
        if let Err(err) = sqlx::Executor::execute(&mut conn, "COMMIT").await {
            drop(conn);
            drop(paused);
            return Err(err.into());
        }
        drop(conn);
        self.update_job("finish", None);
        if let Err(err) = self.services.reload().await {
            tracing::warn!(error = %err, "настройки после восстановления не перечитаны");
        }
        let _ = self.services.notify_settings_changed().await;
        let mut warnings = preview.warnings.clone();
        warnings.extend(report.warnings.clone());
        let secrets = report.secrets.clone();
        if secrets.mode == "cleared" && secrets.touched() {
            warnings.push(format!(
                "Секреты из копии не подошли к ключу этого сервера и удалены: {}",
                secrets.secrets.join(", ")
            ));
        }
        let result = serde_json::json!({
            "source": label,
            "created_at": manifest.created_at,
            "app": manifest.app.shown(),
            "counts": manifest.counts,
            "rows": report.rows,
            "migrated": report.migrated.len(),
            "before": before,
            "keys": preview.keys,
            "secrets": secrets,
            "warnings": warnings,
            "remnawave": manifest.remnawave,
            "remna_sync_path": REMNA_SYNC_PATH,
        });
        let mut conn = self.services.store().conn().await?;
        let run_id: i64 = sqlx::query_scalar(
            "INSERT INTO backup_runs (kind, state, file, sha256, admin_id, details, finished_at) \
             VALUES ('restore', 'done', $1, $2, $3, $4, now()) RETURNING id",
        )
        .bind(label)
        .bind(&report.file_sha256)
        .bind(admin.map(|a| a.0))
        .bind(&result)
        .fetch_one(conn.conn())
        .await?;
        conn.bot_state()
            .set(ALERTS_CURSOR, &serde_json::json!(run_id - 1))
            .await?;
        let actor = admin.map(Actor::Admin).unwrap_or(Actor::System);
        conn.audit()
            .record(&audit_entry(
                actor,
                "backups.restored",
                serde_json::json!({
                    "source": label,
                    "created_at": manifest.created_at,
                    "app": manifest.app.shown(),
                    "before": before,
                    "keys": preview.keys,
                }),
                None,
            ))
            .await?;
        drop(conn);
        drop(paused);
        Ok(result)
    }

    pub async fn alerts_since(&self, cursor: i64) -> ServiceResult<(Vec<(i64, String)>, i64)> {
        let mut conn = self.services.store().conn().await?;
        let rows: Vec<RunRow> = sqlx::query_as(&format!(
            "SELECT {RUN_COLUMNS} FROM backup_runs WHERE id > $1 AND finished_at IS NOT NULL ORDER BY id LIMIT 200"
        ))
        .bind(cursor)
        .fetch_all(conn.conn())
        .await?;
        let running: Option<i64> = sqlx::query_scalar(
            "SELECT min(id) FROM backup_runs WHERE id > $1 AND finished_at IS NULL",
        )
        .bind(cursor)
        .fetch_one(conn.conn())
        .await?;
        drop(conn);
        let mut out = Vec::new();
        let mut last = cursor;
        for row in rows {
            if running.is_some_and(|r| row.id > r) {
                break;
            }
            last = row.id;
            let file = row.file.clone().unwrap_or_default();
            let kind = RunKind::parse(&row.kind);
            if row.state == "failed" {
                let text = match kind {
                    Some(RunKind::Restore) => format!(
                        "<b>Восстановление из копии не прошло</b>\n{}",
                        escape_html(row.error.as_deref().unwrap_or("ошибка"))
                    ),
                    _ => format!(
                        "<b>Резервная копия не сделана</b>\n{}",
                        escape_html(row.error.as_deref().unwrap_or("ошибка"))
                    ),
                };
                out.push((row.id, text));
                continue;
            }
            if kind == Some(RunKind::Restore) {
                out.push((
                    row.id,
                    format!(
                        "<b>Панель восстановлена из копии</b>\n{}",
                        escape_html(&file)
                    ),
                ));
                continue;
            }
            for (key, title) in [("telegram", "в Telegram"), ("s3", "в S3")] {
                let delivery: Option<Delivery> = row
                    .details
                    .get(key)
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                if let Some(delivery) = delivery.filter(|d| d.state == "failed") {
                    out.push((
                        row.id,
                        format!(
                            "<b>Копия не отправлена {title}</b>\n{}: {}",
                            escape_html(&file),
                            escape_html(delivery.error.as_deref().unwrap_or("ошибка"))
                        ),
                    ));
                }
            }
        }
        Ok((out, last))
    }
}

pub fn list_files(dir: &Path) -> Vec<(String, NameInfo, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, NameInfo, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let info = format::parse_name(&name)?;
            let meta = entry.metadata().ok()?;
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            meta.is_file().then_some((name, info, meta.len(), modified))
        })
        .collect();
    out.sort_by(|a, b| {
        b.1.at
            .cmp(&a.1.at)
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| b.0.cmp(&a.0))
    });
    out.into_iter()
        .map(|(name, info, size, _)| (name, info, size))
        .collect()
}
