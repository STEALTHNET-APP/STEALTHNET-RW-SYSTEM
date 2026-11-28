use super::manifest::{Manifest, Problem, Run, Volume};
use super::package::{self, Signature};
use super::runtime::{
    self, Action, Availability, ContainerView, HelperState, JobView, Marker, Request,
};
use super::store::{self, ModuleRow};
use super::{
    Card, Credentials, ExtModulesOps, Preview, build_preview, carry_config, installer, manager,
    missing, problem, rejected, setting_key, sorted,
};
use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sn_domain::access::{AdminPrincipal, AdminRole, Permission};
use sn_store::Db;
use std::path::PathBuf;
use uuid::Uuid;

const HEALTH_STALE_SECONDS: i64 = 180;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallInfo {
    pub owner: bool,
    pub totp_required: bool,
    pub packages: HelperState,
    pub reason: String,
    pub processor: Option<&'static str>,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunView {
    pub port: u16,
    pub memory_mb: u32,
    pub cpus: String,
    pub internet: bool,
    pub volumes: Vec<Volume>,
    pub health_path: String,
    pub events_path: Option<String>,
    pub page_path: Option<String>,
}

impl RunView {
    fn of(manifest: &Manifest, run: &Run) -> Self {
        RunView {
            port: run.port,
            memory_mb: run.memory_mb,
            cpus: run.cpus_text(),
            internet: run.internet,
            volumes: run.volumes.clone(),
            health_path: run.health_path.clone(),
            events_path: run.events_path.clone(),
            page_path: manifest.page.as_ref().and_then(|p| p.path.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageView {
    pub bytes: u64,
    pub sha256: String,
    pub signature: Signature,
    pub platforms: Vec<String>,
    pub processor: &'static str,
    pub image_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackagePreview {
    #[serde(flatten)]
    pub module: Preview,
    pub run: RunView,
    pub previous_run: Option<RunView>,
    pub package: PackageView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackagePreviewResult {
    pub errors: Vec<Problem>,
    pub preview: Option<PackagePreview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PackageInstall {
    pub password: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub trust_unsigned: bool,
    #[serde(default)]
    pub update: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PackageInstalled {
    pub module: Card,
    pub runtime: RuntimeView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuntimeView {
    pub module: String,
    pub state: String,
    pub enabled: bool,
    pub version: String,
    pub run: Option<RunView>,
    pub job: Option<JobView>,
    pub container: Option<ContainerView>,
    pub health: Option<Health>,
    pub package: Option<Value>,
    pub helper: HelperState,
    pub reason: String,
    pub owner: bool,
    pub totp_required: bool,
    pub can_logs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogsView {
    pub lines: Vec<String>,
    pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RemoveRequest {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub erase_data: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reissued {
    pub credentials: Option<Credentials>,
    pub runtime: Option<RuntimeView>,
}

pub(crate) fn owner(actor: &AdminPrincipal) -> ServiceResult<()> {
    if actor.api_key.is_some() || actor.role != AdminRole::Owner {
        return Err(rejected(
            "ext_modules.owner_only",
            "Модули из архива ставит, обновляет и удаляет только владелец",
        ));
    }
    Ok(())
}

fn marker_of(row: &ModuleRow) -> Option<Marker> {
    row.job.clone().and_then(|v| serde_json::from_value(v).ok())
}

fn health_of(row: &ModuleRow) -> Option<Health> {
    row.health
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
}

fn busy_state(action: &str) -> Option<&'static str> {
    match Action::parse(action)? {
        Action::ModuleInstall => Some("installing"),
        Action::ModuleUpdate => Some("updating"),
        Action::ModuleStart | Action::ModuleRestart | Action::ModuleSecrets => Some("starting"),
        Action::ModuleStop => Some("stopping"),
        Action::ModuleRemove => Some("removing"),
        Action::ModuleLogs => None,
    }
}

pub(crate) fn overall(
    enabled: bool,
    job: Option<&JobView>,
    container: Option<&ContainerView>,
    health: Option<&Health>,
    now: DateTime<Utc>,
) -> String {
    if let Some(state) = job
        .filter(|j| j.active())
        .and_then(|j| busy_state(&j.action))
    {
        return state.into();
    }
    if !enabled {
        return "stopped".into();
    }
    let failed = job.is_some_and(|j| j.state == "failed" || j.state == "rolled_back");
    let Some(container) = container else {
        return if failed { "failed" } else { "unknown" }.into();
    };
    match container.state.as_str() {
        "running" => {
            let stale = Duration::seconds(HEALTH_STALE_SECONDS);
            let probed = health.filter(|h| now - h.at < stale).map(|h| (h.at, h.ok));
            let verified = job
                .filter(|j| match j.state.as_str() {
                    "done" => j
                        .steps
                        .iter()
                        .any(|s| s.key == "verify" && s.state == "done"),
                    "rolled_back" => j.restored == Some(true),
                    _ => false,
                })
                .and_then(|j| j.finished_at)
                .filter(|at| now - *at < stale)
                .map(|at| (at, true));
            let latest = match (probed, verified) {
                (Some(p), Some(v)) => Some(if v.0 > p.0 { v } else { p }),
                (p, v) => p.or(v),
            };
            match latest {
                Some((_, true)) => "running".into(),
                Some((_, false)) => "unhealthy".into(),
                None => "starting".into(),
            }
        }
        "restarting" => "restarting".into(),
        "missing" => if failed { "failed" } else { "missing" }.into(),
        _ => "stopped".into(),
    }
}

impl ExtModulesOps<'_> {
    fn helper(&self) -> HelperState {
        runtime::helper_state(self.services.update_config().as_deref())
    }

    fn helper_dir(&self) -> Option<PathBuf> {
        self.services.update_config().and_then(|c| c.dir.clone())
    }

    fn ready_dir(&self) -> ServiceResult<PathBuf> {
        let helper = self.helper();
        if !helper.ready() {
            let reason = helper.reason();
            let mut chars = reason.chars();
            let text: String = match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            };
            return Err(rejected("ext_modules.helper", text));
        }
        self.helper_dir()
            .ok_or_else(|| rejected("ext_modules.helper", "помощника на сервере нет"))
    }

    async fn totp_required(&self, actor: &AdminPrincipal) -> ServiceResult<bool> {
        if actor.api_key.is_some() {
            return Ok(false);
        }
        let mut conn = self.services.store().conn().await?;
        Ok(conn.admins().get(actor.admin_id).await?.totp.is_some())
    }

    pub(crate) fn state_of(&self, row: &ModuleRow) -> Option<String> {
        if !row.managed {
            return None;
        }
        let dir = self.helper_dir();
        let status = dir
            .as_deref()
            .and_then(|d| runtime::read_status(d, row.id()));
        let marker = marker_of(row);
        let waiting = match (&marker, dir.as_deref()) {
            (Some(m), Some(d)) => runtime::pending(d, &m.id),
            _ => false,
        };
        let now = self.services.now();
        let job = runtime::job_view(marker.as_ref(), status.as_ref(), waiting, now);
        let container = runtime::container_view(status.as_ref());
        Some(overall(
            row.enabled,
            job.as_ref(),
            container.as_ref(),
            health_of(row).as_ref(),
            now,
        ))
    }

    async fn submit(
        &self,
        actor: &AdminPrincipal,
        module: &str,
        action: Action,
        package: Option<Uuid>,
        erase: Option<bool>,
        credentials: Option<(&str, &str)>,
    ) -> ServiceResult<Request> {
        let dir = self.ready_dir()?;
        let request = Request {
            id: Uuid::new_v4(),
            action,
            module: module.to_string(),
            package,
            erase,
            admin_id: actor.admin_id.0,
            at: self.services.now(),
        };
        runtime::submit(&dir, &request, credentials)
            .await
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::AlreadyExists {
                    rejected(
                        "ext_modules.busy",
                        "Помощник на сервере выполняет другую заявку. Повторите через минуту",
                    )
                } else {
                    ServiceError::Internal(format!("заявка помощнику не записана: {err}"))
                }
            })?;
        Ok(request)
    }

    async fn withdraw(&self, request: &Request) {
        if let Some(dir) = self.helper_dir() {
            runtime::withdraw(&dir, &request.id).await;
        }
    }

    pub async fn install_info(&self, actor: &AdminPrincipal) -> ServiceResult<InstallInfo> {
        if actor.api_key.is_some() || !actor.can(Permission::Modules) {
            return Err(ServiceError::Forbidden);
        }
        let packages = self.helper();
        Ok(InstallInfo {
            owner: actor.role == AdminRole::Owner,
            totp_required: self.totp_required(actor).await?,
            reason: packages.reason(),
            packages,
            processor: package::host_platform().map(package::processor),
            max_bytes: package::PACKAGE_MAX_BYTES,
        })
    }

    pub fn upload_path(&self, actor: &AdminPrincipal, upload: &Uuid) -> ServiceResult<PathBuf> {
        owner(actor)?;
        let dir = self.ready_dir()?;
        Ok(runtime::upload_path(&dir, upload))
    }

    pub fn cleanup_uploads(&self) {
        if let Some(dir) = self.helper_dir() {
            runtime::cleanup_uploads(&dir);
        }
    }

    async fn inspect_upload(
        &self,
        upload: &Uuid,
    ) -> ServiceResult<Result<package::Inspected, Vec<Problem>>> {
        let dir = self.ready_dir()?;
        let path = runtime::upload_path(&dir, upload);
        if !path.is_file() {
            return Err(rejected(
                "ext_modules.upload_gone",
                "Загруженный архив не найден: выберите его ещё раз",
            ));
        }
        let platform = package::host_platform().ok_or_else(|| {
            rejected(
                "ext_modules.platform",
                "Процессор этого сервера не поддерживается",
            )
        })?;
        tokio::task::spawn_blocking(move || package::inspect(&path, platform, package::TRUSTED))
            .await
            .map_err(|e| ServiceError::Internal(format!("архив не проверен: {e}")))
    }

    fn package_problems(
        &self,
        actor: &AdminPrincipal,
        manifest: &Manifest,
        update: Option<&str>,
        existing: Option<&ModuleRow>,
    ) -> Vec<Problem> {
        let mut errors = Vec::new();
        if let Some(expected) = update
            && expected != manifest.id
        {
            errors.push(problem(
                "id",
                format!(
                    "в архиве другой модуль: «{}», а обновляется «{expected}»",
                    manifest.id
                ),
            ));
        }
        if let Some(row) = existing.filter(|row| !row.managed) {
            errors.push(problem(
                "id",
                format!(
                    "модуль «{}» уже подключён файлом описания: удалите его, чтобы поставить из архива",
                    row.manifest.name
                ),
            ));
        }
        errors.extend(self.rights_problems(actor, manifest));
        errors
    }

    pub async fn check_package(
        &self,
        actor: &AdminPrincipal,
        upload: &Uuid,
        update: Option<&str>,
    ) -> ServiceResult<PackagePreviewResult> {
        owner(actor)?;
        let inspected = match self.inspect_upload(upload).await? {
            Ok(inspected) => inspected,
            Err(errors) => {
                return Ok(PackagePreviewResult {
                    errors,
                    preview: None,
                });
            }
        };
        let existing = {
            let mut conn = self.services.store().conn().await?;
            store::get(&mut conn, &inspected.manifest.id).await?
        };
        let errors = self.package_problems(actor, &inspected.manifest, update, existing.as_ref());
        if !errors.is_empty() {
            return Ok(PackagePreviewResult {
                errors,
                preview: None,
            });
        }
        let Some(run) = inspected.manifest.run.clone() else {
            return Err(rejected("ext_modules.package", "в архиве нет раздела run"));
        };
        let previous_run = existing.as_ref().and_then(|row| {
            row.manifest
                .run
                .as_ref()
                .map(|r| RunView::of(&row.manifest, r))
        });
        let run_view = RunView::of(&inspected.manifest, &run);
        let package = PackageView {
            bytes: inspected.archive_bytes,
            sha256: inspected.archive_sha256.clone(),
            signature: inspected.signature.clone(),
            platforms: inspected.platforms.clone(),
            processor: package::processor(&inspected.image.platform),
            image_bytes: inspected.image.bytes,
        };
        Ok(PackagePreviewResult {
            errors,
            preview: Some(PackagePreview {
                module: build_preview(inspected.manifest, existing.as_ref()),
                run: run_view,
                previous_run,
                package,
            }),
        })
    }

    pub async fn discard_upload(&self, actor: &AdminPrincipal, upload: &Uuid) -> ServiceResult<()> {
        owner(actor)?;
        if let Some(dir) = self.helper_dir() {
            let _ = tokio::fs::remove_file(runtime::upload_path(&dir, upload)).await;
        }
        Ok(())
    }

    pub async fn install_package(
        &self,
        actor: &AdminPrincipal,
        upload: &Uuid,
        request: &PackageInstall,
    ) -> ServiceResult<PackageInstalled> {
        owner(actor)?;
        let helper = self.helper();
        if !helper.ready() {
            return Err(rejected(
                "ext_modules.helper",
                format!("Архив модуля не поставить: {}", helper.reason()),
            ));
        }
        if helper.busy {
            return Err(rejected(
                "ext_modules.busy",
                "Помощник на сервере выполняет другую заявку. Повторите через минуту",
            ));
        }
        self.services
            .auth()
            .confirm_admin(actor.admin_id, &request.password, request.code.as_deref())
            .await?;
        let inspected = self.inspect_upload(upload).await?.map_err(|problems| {
            rejected(
                "ext_modules.package",
                format!(
                    "Архив не прошёл проверку. {}",
                    package::first_message(&problems)
                ),
            )
        })?;
        let manifest = inspected.manifest.clone();
        if !inspected.signature.verified() && !request.trust_unsigned {
            return Err(rejected(
                "ext_modules.untrusted",
                "Архив не подписан STEALTHNET: отметьте, что получили его от автора и доверяете ему",
            ));
        }
        if sorted(&request.permissions) != manifest.permissions
            || sorted(&request.events) != sorted(&manifest.events)
        {
            return Err(rejected(
                "ext_modules.confirm",
                "Архив отличается от того, что вы подтвердили. Загрузите его ещё раз и проверьте права",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        let existing = store::lock(&mut tx, &manifest.id).await?;
        if let Some(first) = self
            .package_problems(
                actor,
                &manifest,
                request.update.as_deref(),
                existing.as_ref(),
            )
            .into_iter()
            .next()
        {
            return Err(rejected("ext_modules.package", first.message));
        }
        let now = self.services.now();
        let package_info = serde_json::json!({
            "sha256": inspected.archive_sha256,
            "bytes": inspected.archive_bytes,
            "image_sha256": inspected.image.sha256,
            "image_bytes": inspected.image.bytes,
            "platforms": inspected.platforms,
            "signature": inspected.signature,
            "installed_at": now,
            "admin_id": actor.admin_id.0,
        });
        let run = manifest.run.clone().unwrap_or_else(|| Run {
            port: 0,
            events_path: None,
            health_path: String::new(),
            inbound_path: None,
            memory_mb: 0,
            cpu_tenths: 0,
            internet: false,
            volumes: Vec::new(),
            images: Vec::new(),
        });
        let details = serde_json::json!({
            "module": manifest.id,
            "version": manifest.version,
            "from_version": existing.as_ref().map(|r| r.manifest.version.clone()),
            "sha256": inspected.archive_sha256,
            "image": package::image_tag(&manifest.id, &inspected.image.sha256),
            "signature": inspected.signature,
            "permissions": manifest.permissions,
            "events": manifest.events,
            "memory_mb": run.memory_mb,
            "cpus": run.cpus_text(),
            "internet": run.internet,
            "volumes": run.volumes,
        });
        let mut pending: Option<(Value, Value)> = None;
        let (action, credentials) = match existing {
            None => {
                let (key_id, token) = self.issue_key(&mut tx, actor, &manifest).await?;
                let secret = self.issue_secret(&mut tx, actor, &manifest.id).await?;
                store::insert_managed(
                    &mut tx,
                    &manifest,
                    key_id,
                    &package_info,
                    &Value::Null,
                    actor.admin_id,
                )
                .await?;
                tx.audit()
                    .record(&Actor::Admin(actor.admin_id).audit(
                        "ext_module.package_installed",
                        "ext_module",
                        None,
                        serde_json::json!({ "api_key": key_id, "details": details }),
                    ))
                    .await?;
                (Action::ModuleInstall, Some((token, secret)))
            }
            Some(row) => {
                pending = Some((
                    row.package.clone().unwrap_or(Value::Null),
                    serde_json::to_value(&manifest)
                        .map_err(|e| ServiceError::Internal(e.to_string()))?,
                ));
                tx.audit()
                    .record(&Actor::Admin(actor.admin_id).audit(
                        "ext_module.package_updated",
                        "ext_module",
                        None,
                        serde_json::json!({ "details": details }),
                    ))
                    .await?;
                (Action::ModuleUpdate, None)
            }
        };
        let submitted = self
            .submit(
                actor,
                &manifest.id,
                action,
                Some(*upload),
                None,
                credentials
                    .as_ref()
                    .map(|(key, secret)| (key.as_str(), secret.as_str())),
            )
            .await?;
        let finished = async {
            store::set_job(&mut tx, &manifest.id, &submitted.marker()).await?;
            if let Some((current, next)) = pending {
                let mut current = match current {
                    Value::Object(map) => map,
                    _ => serde_json::Map::new(),
                };
                current.insert(
                    "pending".into(),
                    serde_json::json!({
                        "request": submitted.id,
                        "manifest": next,
                        "package": package_info,
                    }),
                );
                store::set_package(&mut tx, &manifest.id, &Value::Object(current)).await?;
            }
            tx.commit().await?;
            Ok::<(), ServiceError>(())
        }
        .await;
        if let Err(err) = finished {
            self.withdraw(&submitted).await;
            return Err(err);
        }
        Ok(PackageInstalled {
            module: self.card(&manifest.id).await?,
            runtime: self.runtime_view(actor, &manifest.id).await?,
        })
    }

    pub(crate) async fn settle(&self, id: &str) -> ServiceResult<()> {
        let row = {
            let mut conn = self.services.store().conn().await?;
            store::get(&mut conn, id).await?
        };
        let Some(row) = row.filter(|row| row.managed) else {
            return Ok(());
        };
        let Some(pending) = row.package.as_ref().and_then(|p| p.get("pending")).cloned() else {
            return Ok(());
        };
        let request = pending
            .get("request")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let dir = self.helper_dir();
        let status = dir.as_deref().and_then(|d| runtime::read_status(d, id));
        let marker = marker_of(&row);
        let waiting = dir
            .as_deref()
            .is_some_and(|d| runtime::pending(d, &request));
        let now = self.services.now();
        let outcome = match status
            .as_ref()
            .and_then(|s| s.job.as_ref())
            .filter(|job| job.id == request)
        {
            Some(job) if job.state == "done" => Some(true),
            Some(job) if matches!(job.state.as_str(), "failed" | "rolled_back") => Some(false),
            Some(_) => None,
            None => {
                let current = marker.as_ref().filter(|m| m.id == request);
                let stale = current.is_none()
                    || runtime::job_view(current, None, waiting, now)
                        .is_some_and(|job| job.state == "failed");
                stale.then_some(false)
            }
        };
        let Some(applied) = outcome else {
            return Ok(());
        };
        let mut tx = self.services.store().begin().await?;
        let Some(row) = store::lock(&mut tx, id).await? else {
            return Ok(());
        };
        let still = row
            .package
            .as_ref()
            .and_then(|p| p.get("pending"))
            .and_then(|p| p.get("request"))
            .and_then(Value::as_str)
            == Some(request.as_str());
        if !still {
            return Ok(());
        }
        let mut current = match row.package.clone() {
            Some(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        current.remove("pending");
        let next_package = pending.get("package").cloned().unwrap_or(Value::Null);
        if applied {
            let manifest: Manifest =
                serde_json::from_value(pending.get("manifest").cloned().unwrap_or(Value::Null))
                    .map_err(|e| {
                        ServiceError::Internal(format!("новое описание модуля не читается: {e}"))
                    })?;
            let (config, dropped) = carry_config(&row, &manifest.settings);
            for key in dropped {
                tx.secrets().delete(&setting_key(id, &key)).await?;
            }
            store::update(&mut tx, &manifest, &config, row.installed_by).await?;
            if row.manifest.permissions != manifest.permissions
                && let Some(key_id) = row.api_key_id
            {
                tx.api_keys()
                    .set_permissions(key_id, &manifest.permissions)
                    .await?;
            }
            store::set_package(&mut tx, id, &next_package).await?;
        } else {
            store::set_package(&mut tx, id, &Value::Object(current)).await?;
        }
        tx.commit().await?;
        super::proxy::forget(id);
        Ok(())
    }

    async fn runtime_view(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<RuntimeView> {
        self.settle(id).await?;
        let row = self.row(id).await?;
        if !row.managed {
            return Err(rejected(
                "ext_modules.not_package",
                "Модуль работает на своём сервере: панель его не запускает",
            ));
        }
        let dir = self.helper_dir();
        let status = dir.as_deref().and_then(|d| runtime::read_status(d, id));
        let marker = marker_of(&row);
        let waiting = match (&marker, dir.as_deref()) {
            (Some(m), Some(d)) => runtime::pending(d, &m.id),
            _ => false,
        };
        let now = self.services.now();
        let job = runtime::job_view(marker.as_ref(), status.as_ref(), waiting, now);
        let container = runtime::container_view(status.as_ref());
        let health = health_of(&row);
        let helper = self.helper();
        Ok(RuntimeView {
            module: id.to_string(),
            state: overall(
                row.enabled,
                job.as_ref(),
                container.as_ref(),
                health.as_ref(),
                now,
            ),
            enabled: row.enabled,
            version: row.manifest.version.clone(),
            run: row
                .manifest
                .run
                .as_ref()
                .map(|r| RunView::of(&row.manifest, r)),
            job,
            container,
            health,
            package: row.package.clone(),
            reason: helper.reason(),
            helper,
            owner: actor.role == AdminRole::Owner && actor.api_key.is_none(),
            totp_required: self.totp_required(actor).await?,
            can_logs: actor.can(Permission::Modules) && actor.can(Permission::Settings),
        })
    }

    pub async fn runtime(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<RuntimeView> {
        if actor.api_key.is_some() || !actor.can(Permission::Modules) {
            return Err(ServiceError::Forbidden);
        }
        self.runtime_view(actor, id).await
    }

    async fn managed_row(&self, id: &str) -> ServiceResult<ModuleRow> {
        let row = self.row(id).await?;
        if !row.managed {
            return Err(rejected(
                "ext_modules.not_package",
                "Модуль работает на своём сервере: панель его не запускает",
            ));
        }
        Ok(row)
    }

    async fn request_simple(
        &self,
        actor: &AdminPrincipal,
        id: &str,
        action: Action,
        audit: &str,
    ) -> ServiceResult<RuntimeView> {
        let request = self.submit(actor, id, action, None, None, None).await?;
        let mut tx = self.services.store().begin().await?;
        store::set_job(&mut tx, id, &request.marker()).await?;
        tx.audit()
            .record(&Actor::Admin(actor.admin_id).audit(
                audit,
                "ext_module",
                None,
                serde_json::json!({ "module": id, "request": request.id }),
            ))
            .await?;
        if let Err(err) = tx.commit().await {
            self.withdraw(&request).await;
            return Err(err.into());
        }
        self.runtime_view(actor, id).await
    }

    pub async fn restart(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<RuntimeView> {
        manager(actor)?;
        let row = self.managed_row(id).await?;
        if !row.enabled {
            return Err(rejected(
                "ext_modules.disabled",
                "Модуль выключен: включите его, и он запустится",
            ));
        }
        self.request_simple(actor, id, Action::ModuleRestart, "ext_module.restarted")
            .await
    }

    pub async fn refresh(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<RuntimeView> {
        installer(actor)?;
        self.managed_row(id).await?;
        let request = self
            .submit(actor, id, Action::ModuleLogs, None, None, None)
            .await?;
        let mut conn = self.services.store().conn().await?;
        store::set_job(&mut conn, id, &request.marker()).await?;
        drop(conn);
        self.runtime_view(actor, id).await
    }

    pub async fn logs(&self, actor: &AdminPrincipal, id: &str) -> ServiceResult<LogsView> {
        installer(actor)?;
        self.managed_row(id).await?;
        let found = self
            .helper_dir()
            .and_then(|dir| runtime::read_logs(&dir, id));
        Ok(match found {
            Some((lines, at)) => LogsView { lines, at },
            None => LogsView {
                lines: Vec::new(),
                at: None,
            },
        })
    }

    pub(crate) async fn switch_managed(
        &self,
        actor: &AdminPrincipal,
        id: &str,
        enabled: bool,
    ) -> ServiceResult<()> {
        let action = if enabled {
            Action::ModuleStart
        } else {
            Action::ModuleStop
        };
        let request = self.submit(actor, id, action, None, None, None).await?;
        let mut tx = self.services.store().begin().await?;
        let Some(row) = store::lock(&mut tx, id).await? else {
            self.withdraw(&request).await;
            return Err(missing());
        };
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
                    serde_json::json!({ "module": id, "request": request.id }),
                ))
                .await?;
        }
        store::set_job(&mut tx, id, &request.marker()).await?;
        if let Err(err) = tx.commit().await {
            self.withdraw(&request).await;
            return Err(err.into());
        }
        Ok(())
    }

    pub(crate) async fn remove_managed(
        &self,
        actor: &AdminPrincipal,
        row: &ModuleRow,
        body: &RemoveRequest,
    ) -> ServiceResult<()> {
        owner(actor)?;
        let password = body.password.as_deref().unwrap_or_default();
        if password.is_empty() {
            return Err(rejected(
                "ext_modules.password",
                "Введите свой пароль, чтобы удалить модуль",
            ));
        }
        self.services
            .auth()
            .confirm_admin(actor.admin_id, password, body.code.as_deref())
            .await?;
        let id = row.id();
        let request = self
            .submit(
                actor,
                id,
                Action::ModuleRemove,
                None,
                Some(body.erase_data),
                None,
            )
            .await?;
        let result = async {
            let mut tx = self.services.store().begin().await?;
            if store::lock(&mut tx, id).await?.is_none() {
                return Err(missing());
            }
            let revoked = match row.api_key_id {
                Some(key) => tx.api_keys().revoke(key).await?,
                None => false,
            };
            for key in tx.secrets().keys(&super::secrets_prefix(id)).await? {
                tx.secrets().delete(&key).await?;
            }
            store::delete(&mut tx, id).await?;
            tx.audit()
                .record(&Actor::Admin(actor.admin_id).audit(
                    "ext_module.package_removed",
                    "ext_module",
                    None,
                    serde_json::json!({
                        "module": id,
                        "version": row.manifest.version,
                        "api_key": row.api_key_id,
                        "api_key_revoked": revoked,
                        "erase_data": body.erase_data,
                        "request": request.id,
                    }),
                ))
                .await?;
            tx.commit().await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            self.withdraw(&request).await;
        }
        result
    }

    pub(crate) async fn reissue_managed(
        &self,
        actor: &AdminPrincipal,
        id: &str,
    ) -> ServiceResult<Reissued> {
        let helper = self.helper();
        if !helper.ready() || helper.busy {
            return Err(rejected(
                "ext_modules.helper",
                if helper.busy {
                    "Помощник на сервере выполняет другую заявку. Повторите через минуту"
                        .to_string()
                } else {
                    format!("Ключи модуля не обновить: {}", helper.reason())
                },
            ));
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
        let request = self
            .submit(
                actor,
                id,
                Action::ModuleSecrets,
                None,
                None,
                Some((&token, &secret)),
            )
            .await?;
        store::set_job(&mut tx, id, &request.marker()).await?;
        tx.audit()
            .record(&Actor::Admin(actor.admin_id).audit(
                "ext_module.credentials",
                "ext_module",
                None,
                serde_json::json!({
                    "module": id,
                    "revoked_key": row.api_key_id,
                    "api_key": key_id,
                    "request": request.id,
                }),
            ))
            .await?;
        if let Err(err) = tx.commit().await {
            self.withdraw(&request).await;
            return Err(err.into());
        }
        Ok(Reissued {
            credentials: None,
            runtime: Some(self.runtime_view(actor, id).await?),
        })
    }

    pub async fn settle_all(&self) -> ServiceResult<()> {
        let ids: Vec<String> = {
            let mut conn = self.services.store().conn().await?;
            store::list(&mut conn)
                .await?
                .into_iter()
                .filter(|row| row.managed)
                .map(|row| row.id().to_string())
                .collect()
        };
        for id in ids {
            self.settle(&id).await?;
        }
        Ok(())
    }

    pub async fn probe_health(&self) -> ServiceResult<usize> {
        self.settle_all().await?;
        let rows = {
            let mut conn = self.services.store().conn().await?;
            store::managed_enabled(&mut conn).await?
        };
        let config = self.services.update_config();
        let mut probed = 0;
        for row in rows {
            let Some(run) = row.manifest.run.as_ref() else {
                continue;
            };
            let url = runtime::url(config.as_deref(), row.id(), run.port, &run.health_path);
            let result = super::address::internal_client().get(&url).send().await;
            let error = match result {
                Ok(response) if response.status().is_success() => None,
                Ok(response) => Some(format!(
                    "модуль ответил кодом {}",
                    response.status().as_u16()
                )),
                Err(err) if err.is_timeout() => Some("модуль не ответил за 10 секунд".into()),
                Err(_) => Some("не удалось соединиться с модулем".into()),
            };
            let now = self.services.now();
            let previous = health_of(&row);
            let since = match (&error, previous) {
                (None, _) => None,
                (Some(_), Some(prev)) if !prev.ok => prev.since.or(Some(prev.at)),
                (Some(_), _) => Some(now),
            };
            let health = Health {
                ok: error.is_none(),
                at: now,
                error,
                since,
            };
            let value =
                serde_json::to_value(&health).map_err(|e| ServiceError::Internal(e.to_string()))?;
            let mut conn = self.services.store().conn().await?;
            store::set_health(&mut conn, row.id(), &value).await?;
            probed += 1;
        }
        Ok(probed)
    }

    pub fn helper_state(&self) -> HelperState {
        self.helper()
    }

    pub fn packages_ready(&self) -> bool {
        self.helper().state == Availability::Ready
    }
}
