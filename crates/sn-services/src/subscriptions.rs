use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::remna_sync::{mirror_outdated, same_user, store_user};
use crate::services::Services;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sn_domain::DomainError;
use sn_domain::ids::{ClientId, PeriodId, RemnaUserId, SubscriptionId};
use sn_domain::jobs::JobPayload;
use sn_domain::subscription::{
    AdminStatus, AutoRenewState, PeriodProgress, Subscription, SubscriptionStatus, admin_status,
    period_progress,
};
use sn_remnawave::{
    DateRange, DropConnectionsRequest, HwidDevice, HwidDevices, RemnaError, RemnaUser,
    RemnawaveApi, RevokeRequest, UpdateUserRequest, UserStatus,
};
use sn_store::{ActorKind, AuditEntry, Db, LockSpace, PurchaseApplied, StoreError};
use std::collections::BTreeSet;
use std::future::Future;
use uuid::Uuid;

mod addons;
pub mod traffic;

pub const MAX_ADD_DAYS: u32 = 3650;
pub const MAX_UNDO_STEPS: u32 = 50;
const CONNECTION_POLLS: u32 = 4;
const CONNECTION_POLL_DELAY: std::time::Duration = std::time::Duration::from_millis(700);
const REQUESTS_SHOWN: usize = 20;
const RENEWED_TEMPLATE: &str = "subscription_renewed";
const ADDED_NOTICE_DELAY_SECONDS: i64 = 60;
pub(crate) const ALREADY_DISABLED: &str = "A029";
pub(crate) const ALREADY_ENABLED: &str = "A030";
const HOLDS_BATCH: i64 = 200;

pub mod actions {
    pub const ADD_DAYS: &str = "subscription.add_days";
    pub const ADD_DAYS_UNDONE: &str = "subscription.add_days_undone";
    pub const RESET_TRAFFIC: &str = "subscription.reset_traffic";
    pub const REISSUE_LINK: &str = "subscription.reissue_link";
    pub const DISCONNECT_ALL_DEVICES: &str = "subscription.disconnect_all_devices";
    pub const DISCONNECT_DEVICE: &str = "subscription.disconnect_device";
    pub const CHANGE_LIMITS: &str = "subscription.change_limits";
    pub const BLOCK_ACCESS: &str = "subscription.block_access";
    pub const RESTORE_ACCESS: &str = "subscription.restore_access";
    pub const RELEASE_DEVICES: &str = "subscription.release_devices";
    pub const BLOCKED_DELIVERY: &str = "subscription.blocked_delivery";
    pub const DELETED: &str = "subscription.deleted";
    pub const DEVICE_ADDON: &str = "subscription.device_addon";
    pub const TRAFFIC_ADDON: &str = "subscription.traffic_addon";
    pub const EXTRA_TRAFFIC_SETTLED: &str = "subscription.extra_traffic_settled";
}

pub(crate) async fn with_subscription_lock<T, E, F>(
    services: &Services,
    id: SubscriptionId,
    work: F,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<StoreError>,
{
    let mut guard = services.store().begin().await?;
    guard.lock(LockSpace::Subscription, id.0).await?;
    let result = work.await;
    guard.rollback().await?;
    result
}

pub(crate) async fn disable_remna_user(
    remna: &dyn RemnawaveApi,
    user_id: RemnaUserId,
) -> Result<RemnaUser, RemnaError> {
    match remna.disable_user(user_id).await {
        Err(err) if err.code() == Some(ALREADY_DISABLED) => remna.get_user(user_id).await,
        other => other,
    }
}

pub(crate) async fn enable_remna_user(
    remna: &dyn RemnawaveApi,
    user_id: RemnaUserId,
) -> Result<RemnaUser, RemnaError> {
    match remna.enable_user(user_id).await {
        Err(err) if err.code() == Some(ALREADY_ENABLED) => remna.get_user(user_id).await,
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubscriptionView {
    pub subscription: Subscription,
    pub status: AdminStatus,
    pub progress: Option<PeriodProgress>,
    pub subscription_url: Option<String>,
    pub countries: Vec<String>,
    pub tariff: Option<crate::clients::TariffBrief>,
    pub renewal: Option<crate::renewals::RenewalView>,
}

async fn tariff_brief(
    db: &mut impl sn_store::Db,
    id: Option<sn_domain::ids::TariffId>,
) -> ServiceResult<Option<crate::clients::TariffBrief>> {
    let Some(id) = id else {
        return Ok(None);
    };
    match db.tariffs().get(id).await {
        Ok(t) => Ok(Some(crate::clients::TariffBrief {
            id: t.id,
            category_id: t.category_id,
            name: t.name,
            on_sale: t.on_sale,
            devices: t.devices,
            traffic: t.traffic,
        })),
        Err(sn_store::StoreError::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdminSubscriptionView {
    #[serde(flatten)]
    pub view: SubscriptionView,
    pub squads: Vec<String>,
    pub last_node: Option<String>,
    pub traffic_30d_bytes: Option<u64>,
    pub remna_reachable: bool,
    pub remna_missing_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionView {
    pub node: String,
    pub country_code: String,
    pub ip: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionsView {
    Pending { job_id: String },
    Done { connections: Vec<ConnectionView> },
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestView {
    pub at: DateTime<Utc>,
    pub response: String,
    pub ip: Option<String>,
    pub app: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceView {
    pub hwid: String,
    pub title: String,
    pub platform: Option<String>,
    pub os_version: Option<String>,
    pub app: Option<String>,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitsChange {
    #[serde(default, deserialize_with = "present_or_null")]
    pub traffic_limit_bytes: Option<Option<u64>>,
    #[serde(default)]
    pub extra_devices: Option<u32>,
    #[serde(default)]
    pub devices: Option<u32>,
}

pub const MAX_DEVICES: u32 = 100;

fn present_or_null<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn actor_fields(actor: Actor) -> (ActorKind, Option<i64>) {
    match actor {
        Actor::Admin(id) => (ActorKind::Admin, Some(id.0)),
        Actor::Client(id) => (ActorKind::Client, Some(id.0)),
        Actor::System => (ActorKind::System, None),
    }
}

fn linked_user(subscription: &Subscription) -> ServiceResult<RemnaUserId> {
    subscription
        .remna
        .as_ref()
        .map(|link| link.user_id)
        .ok_or(ServiceError::Domain(DomainError::NoRemnaUser))
}

fn ensure_live(subscription: &Subscription) -> ServiceResult<()> {
    if subscription.ended_at.is_some() {
        return Err(ServiceError::rejected(
            "subscription.ended",
            "подписка завершена",
        ));
    }
    Ok(())
}

fn noted(mut details: serde_json::Value, note: Option<&serde_json::Value>) -> serde_json::Value {
    if let (serde_json::Value::Object(map), Some(serde_json::Value::Object(extra))) =
        (&mut details, note)
    {
        for (key, value) in extra {
            map.insert(key.clone(), value.clone());
        }
    }
    details
}

fn staff_only(actor: Actor) -> ServiceResult<()> {
    match actor {
        Actor::Client(_) => Err(ServiceError::Forbidden),
        Actor::Admin(_) | Actor::System => Ok(()),
    }
}

fn devices_total(devices: &HwidDevices) -> u32 {
    u32::try_from(devices.total.max(0)).unwrap_or(u32::MAX)
}

fn status_of(user: &RemnaUser) -> SubscriptionStatus {
    user.status.into()
}

fn is_model_code(model: &str) -> bool {
    let Some((head, tail)) = model.split_once(',') else {
        return false;
    };
    let letters = head.trim_end_matches(|c: char| c.is_ascii_digit());
    !letters.is_empty()
        && letters.len() < head.len()
        && letters.chars().all(|c| c.is_ascii_alphabetic())
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit())
}

fn platform_title(platform: Option<&str>) -> String {
    let platform = platform.map(str::trim).unwrap_or_default();
    let title = match platform.to_ascii_lowercase().as_str() {
        "ios" | "iphone" => "iPhone",
        "ipados" | "ipad" => "iPad",
        "android" => "Android",
        "androidtv" | "android_tv" | "android tv" => "Android TV",
        "macos" | "mac" | "darwin" | "osx" => "Mac",
        "windows" | "win32" | "win" => "Windows",
        "linux" => "Linux",
        "tvos" | "appletv" | "apple tv" => "Apple TV",
        "" => "Устройство",
        _ => return platform.to_string(),
    };
    title.to_string()
}

pub fn device_title(device: &HwidDevice) -> String {
    match device
        .device_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty() && !is_model_code(model))
    {
        Some(model) => model.to_string(),
        None => platform_title(device.platform.as_deref()),
    }
}

pub fn app_name(user_agent: Option<&str>) -> Option<String> {
    let token = user_agent?.split_whitespace().next()?;
    let (name, version) = match token.split_once('/') {
        Some((name, version)) => (name.trim(), Some(version)),
        None => (token.trim(), None),
    };
    if name.is_empty() || name.eq_ignore_ascii_case("mozilla") {
        return None;
    }
    let version = version
        .map(|v| v.trim_start_matches(['v', 'V']))
        .and_then(|v| v.split(['/', '-', '+', '(', ';']).next())
        .map(|v| {
            let mut parts = v.split('.').filter(|part| !part.is_empty());
            match (parts.next(), parts.next()) {
                (Some(major), Some(minor)) => format!("{major}.{minor}"),
                (Some(major), None) => major.to_string(),
                _ => String::new(),
            }
        })
        .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()));
    Some(match version {
        Some(version) => format!("{name} {version}"),
        None => name.to_string(),
    })
}

pub fn device_view(device: &HwidDevice) -> DeviceView {
    DeviceView {
        hwid: device.hwid.clone(),
        title: device_title(device),
        platform: device.platform.clone(),
        os_version: device.os_version.clone(),
        app: app_name(device.user_agent.as_deref()),
        added_at: device.created_at,
    }
}

async fn confirm<F>(
    remna: &dyn RemnawaveApi,
    user_id: RemnaUserId,
    err: RemnaError,
    applied: F,
) -> ServiceResult<RemnaUser>
where
    F: Fn(&RemnaUser) -> bool,
{
    if !err.outcome_unknown() {
        return Err(err.into());
    }
    match remna.get_user(user_id).await {
        Ok(user) if applied(&user) => Ok(user),
        _ => Err(err.into()),
    }
}

async fn confirm_devices<F>(
    remna: &dyn RemnawaveApi,
    user_id: RemnaUserId,
    err: RemnaError,
    applied: F,
) -> ServiceResult<HwidDevices>
where
    F: Fn(&HwidDevices) -> bool,
{
    if !err.outcome_unknown() {
        return Err(err.into());
    }
    match remna.user_devices(user_id).await {
        Ok(devices) if applied(&devices) => Ok(devices),
        _ => Err(err.into()),
    }
}

pub(crate) async fn drop_everywhere(
    remna: &dyn RemnawaveApi,
    user_ids: Vec<RemnaUserId>,
) -> Result<(), RemnaError> {
    let request = DropConnectionsRequest::users_everywhere(user_ids);
    match remna.drop_connections(&request).await {
        Err(err) if err.outcome_unknown() => remna.drop_connections(&request).await,
        other => other,
    }
}

struct Change<'a> {
    subscription: &'a Subscription,
    actor: Actor,
    action: &'static str,
    details: serde_json::Value,
    user: Option<&'a RemnaUser>,
    devices: Option<u32>,
    purchase: Option<PurchaseApplied>,
}

struct DevicesCut {
    remaining: Option<u32>,
    failed: Vec<(&'static str, ServiceError)>,
}

struct Trimmed {
    removed: Vec<String>,
    left: Option<u32>,
    errors: Vec<ServiceError>,
}

enum Restored {
    Missing,
    Untouched(RemnaUser),
    Enabled(RemnaUser),
}

async fn trim_devices(remna: &dyn RemnawaveApi, user_id: RemnaUserId, limit: i64) -> Trimmed {
    let limit = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    let mut trimmed = Trimmed {
        removed: Vec::new(),
        left: None,
        errors: Vec::new(),
    };
    match remna.user_devices(user_id).await {
        Ok(listed) => {
            let mut left = devices_total(&listed);
            let mut devices = listed.devices;
            devices.sort_by_key(|d| (d.updated_at, d.created_at));
            let excess = devices.len().saturating_sub(limit);
            for device in devices.iter().take(excess) {
                match remna.delete_user_device(user_id, &device.hwid).await {
                    Ok(rest) => {
                        left = devices_total(&rest);
                        trimmed.removed.push(device.hwid.clone());
                    }
                    Err(err) => {
                        trimmed.errors.push(err.into());
                        break;
                    }
                }
            }
            trimmed.left = Some(left);
        }
        Err(err) => trimmed.errors.push(err.into()),
    }
    trimmed
}

pub struct SubscriptionOps<'a> {
    services: &'a Services,
}

impl<'a> SubscriptionOps<'a> {
    pub fn new(services: &'a Services) -> Self {
        SubscriptionOps { services }
    }

    async fn owned(&self, id: SubscriptionId, actor: Actor) -> ServiceResult<Subscription> {
        let mut conn = self.services.store().conn().await?;
        let subscription = conn.subscriptions().get(id).await?;
        if let Actor::Client(client_id) = actor
            && subscription.client_id != client_id
        {
            return Err(ServiceError::NotFound("подписка"));
        }
        Ok(subscription)
    }

    async fn mark_missing(&self, id: SubscriptionId) -> ServiceResult<()> {
        let mut conn = self.services.store().conn().await?;
        conn.subscriptions().mark_missing(id, true).await?;
        Ok(())
    }

    async fn missing_since(&self, id: SubscriptionId) -> ServiceResult<Option<DateTime<Utc>>> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.subscriptions().missing_since(id).await?)
    }

    async fn fetch_user(
        &self,
        remna: &dyn RemnawaveApi,
        subscription: &Subscription,
        user_id: RemnaUserId,
    ) -> ServiceResult<RemnaUser> {
        match remna.get_user(user_id).await {
            Ok(user) => Ok(user),
            Err(err) if err.is_not_found() => {
                self.mark_missing(subscription.id).await?;
                Err(ServiceError::NotFound("пользователь Remnawave"))
            }
            Err(err) => Err(err.into()),
        }
    }

    async fn record(&self, change: Change<'_>) -> ServiceResult<()> {
        let subscription = change.subscription;
        let mut tx = self.services.store().begin().await?;
        if let Some(purchase) = &change.purchase {
            tx.subscriptions()
                .apply_purchase(subscription.id, purchase)
                .await?;
        }
        match change.user {
            Some(user) => {
                store_user(
                    &mut tx,
                    subscription.id,
                    user,
                    change.devices,
                    self.services.now(),
                )
                .await?;
            }
            None => {
                if let Some(count) = change.devices {
                    tx.subscriptions()
                        .set_devices_count(subscription.id, count)
                        .await?;
                }
            }
        }
        let mut details = change.details;
        if let Some(object) = details.as_object_mut() {
            object.insert("client_id".into(), json!(subscription.client_id));
            object.insert("at".into(), json!(self.services.now()));
        }
        let (actor_kind, actor_id) = actor_fields(change.actor);
        tx.audit()
            .record(&AuditEntry {
                actor_kind,
                actor_id,
                action: change.action.to_string(),
                entity_kind: Some("subscription".into()),
                entity_id: Some(subscription.id.0),
                details,
                ip: None,
            })
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn view_of(
        &self,
        id: SubscriptionId,
        user: Option<&RemnaUser>,
        countries: Vec<String>,
    ) -> ServiceResult<SubscriptionView> {
        let subscription = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions().get(id).await?
        };
        self.view_from(subscription, user, countries).await
    }

    async fn view_from(
        &self,
        subscription: Subscription,
        user: Option<&RemnaUser>,
        countries: Vec<String>,
    ) -> ServiceResult<SubscriptionView> {
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(subscription.client_id).await?;
        let now = self.services.now();
        let tariff = tariff_brief(&mut conn, subscription.tariff_id).await?;
        let renewal = self
            .services
            .renewals()
            .view_in(&mut conn, subscription.id)
            .await?;
        Ok(SubscriptionView {
            status: admin_status(Some(&subscription), client.is_blocked(), now),
            progress: period_progress(&subscription, now),
            subscription_url: user
                .map(|u| u.subscription_url.clone())
                .filter(|url| !url.is_empty()),
            countries,
            tariff,
            renewal,
            subscription,
        })
    }

    async fn cut_devices(&self, remna: &dyn RemnawaveApi, user_id: RemnaUserId) -> DevicesCut {
        let mut failed = Vec::new();
        let remaining = match remna.delete_all_user_devices(user_id).await {
            Ok(devices) => Some(devices_total(&devices)),
            Err(err) => match confirm_devices(remna, user_id, err, |d| d.total == 0).await {
                Ok(devices) => Some(devices_total(&devices)),
                Err(err) => {
                    failed.push(("hwid_delete_all", err));
                    None
                }
            },
        };
        if let Err(err) = drop_everywhere(remna, vec![user_id]).await {
            failed.push(("connections_drop", err.into()));
        }
        DevicesCut { remaining, failed }
    }

    pub async fn list_for_client(
        &self,
        client_id: ClientId,
    ) -> ServiceResult<Vec<SubscriptionView>> {
        let mut conn = self.services.store().conn().await?;
        let client = conn.clients().get(client_id).await?;
        let now = self.services.now();
        let subscriptions = conn.subscriptions().by_client(client_id).await?;
        let mut views = Vec::with_capacity(subscriptions.len());
        for subscription in subscriptions {
            let tariff = tariff_brief(&mut conn, subscription.tariff_id).await?;
            let renewal = self
                .services
                .renewals()
                .view_in(&mut conn, subscription.id)
                .await?;
            views.push(SubscriptionView {
                status: admin_status(Some(&subscription), client.is_blocked(), now),
                progress: period_progress(&subscription, now),
                subscription_url: None,
                countries: Vec::new(),
                tariff,
                renewal,
                subscription,
            });
        }
        Ok(views)
    }

    pub async fn view(
        &self,
        id: SubscriptionId,
        actor: Actor,
        refresh: bool,
    ) -> ServiceResult<SubscriptionView> {
        let subscription = self.owned(id, actor).await?;
        let link = match &subscription.remna {
            Some(link) if refresh => link.clone(),
            _ => return self.view_from(subscription, None, Vec::new()).await,
        };
        let remna = self.services.remna()?;
        let user = match remna.get_user(link.user_id).await {
            Ok(user) => user,
            Err(err) if err.is_not_found() => {
                self.mark_missing(id).await?;
                return self.view_of(id, None, Vec::new()).await;
            }
            Err(err) => return Err(err.into()),
        };
        let squads: Vec<Uuid> = user
            .active_internal_squads
            .iter()
            .map(|squad| squad.uuid)
            .collect();
        let countries =
            crate::squad_countries::union(&squads, &self.services.squad_countries(&squads).await);
        if !mirror_outdated(&subscription, &user, self.services.now()) {
            return self.view_from(subscription, Some(&user), countries).await;
        }
        let mut tx = self.services.store().begin().await?;
        store_user(&mut tx, id, &user, None, self.services.now()).await?;
        tx.commit().await?;
        self.view_of(id, Some(&user), countries).await
    }

    pub async fn admin_view(
        &self,
        id: SubscriptionId,
        actor: Actor,
    ) -> ServiceResult<AdminSubscriptionView> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        let stored = |reachable: bool| async move {
            let view = self.view_of(id, None, Vec::new()).await?;
            Ok::<AdminSubscriptionView, ServiceError>(AdminSubscriptionView {
                view,
                squads: Vec::new(),
                last_node: None,
                traffic_30d_bytes: None,
                remna_reachable: reachable,
                remna_missing_at: self.missing_since(id).await?,
            })
        };
        let Some(link) = &subscription.remna else {
            return stored(true).await;
        };
        let remna = self.services.remna()?;
        let user = match remna.get_user(link.user_id).await {
            Ok(user) if same_user(link, &user) => user,
            Ok(_) => {
                self.mark_missing(id).await?;
                return stored(true).await;
            }
            Err(err) if err.is_not_found() => {
                self.mark_missing(id).await?;
                return stored(true).await;
            }
            Err(err) => {
                tracing::warn!(subscription = %id, error = %err, "Remnawave не ответила, карточка на сохранённых данных");
                return stored(false).await;
            }
        };
        {
            let mut tx = self.services.store().begin().await?;
            tx.subscriptions().mark_missing(id, false).await?;
            store_user(&mut tx, id, &user, None, self.services.now()).await?;
            tx.commit().await?;
        }
        let accessible = match remna.user_accessible_nodes(link.user_id).await {
            Ok(nodes) => Some(nodes),
            Err(err) => {
                tracing::warn!(error = %err, "ноды подписки не получены");
                None
            }
        };
        let countries = accessible
            .as_ref()
            .map(|nodes| {
                nodes
                    .country_codes()
                    .into_iter()
                    .filter(|code| !code.eq_ignore_ascii_case("XX"))
                    .collect()
            })
            .unwrap_or_default();
        let last_uuid = user.user_traffic.last_connected_node_uuid;
        let mut last_node = last_uuid.and_then(|uuid| {
            accessible
                .as_ref()?
                .active_nodes
                .iter()
                .find(|node| node.uuid == uuid)
                .map(|node| node.node_name.clone())
        });
        if last_node.is_none()
            && let Some(uuid) = last_uuid
            && let Ok(nodes) = remna.nodes().await
        {
            last_node = nodes
                .into_iter()
                .find(|node| node.uuid == uuid)
                .map(|node| node.name);
        }
        let today = self.services.now().date_naive();
        let range = DateRange {
            start: today
                .checked_sub_days(chrono::Days::new(29))
                .unwrap_or(today),
            end: today,
        };
        let traffic_30d_bytes = match remna.user_usage(link.user_id, range).await {
            Ok(usage) => Some(usage.total_bytes()),
            Err(err) => {
                tracing::warn!(error = %err, "трафик подписки за 30 дней не получен");
                None
            }
        };
        let squads = user
            .active_internal_squads
            .iter()
            .map(|squad| squad.name.clone())
            .collect();
        Ok(AdminSubscriptionView {
            view: self.view_of(id, Some(&user), countries).await?,
            squads,
            last_node,
            traffic_30d_bytes,
            remna_reachable: true,
            remna_missing_at: None,
        })
    }

    pub async fn connections(
        &self,
        id: SubscriptionId,
        job_id: Option<&str>,
        actor: Actor,
    ) -> ServiceResult<ConnectionsView> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        ensure_live(&subscription)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let job_id = match job_id {
            Some(job)
                if !job.is_empty()
                    && job.len() <= 100
                    && job
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
            {
                job.to_string()
            }
            Some(_) => {
                return Err(ServiceError::rejected(
                    "subscription.connections_job",
                    "неверный номер запроса подключений",
                ));
            }
            None => remna.request_user_connections(user_id).await?.job_id,
        };
        for attempt in 0..CONNECTION_POLLS {
            let status = remna.user_connections(&job_id).await?;
            if status.is_failed {
                return Ok(ConnectionsView::Failed);
            }
            if status.is_completed {
                let nodes = status.result.map(|r| r.nodes).unwrap_or_default();
                let connections = nodes
                    .into_iter()
                    .flat_map(|node| {
                        let name = node.node_name;
                        let country = node.country_code;
                        node.ips.into_iter().map(move |ip| ConnectionView {
                            node: name.clone(),
                            country_code: country.clone(),
                            ip: ip.ip,
                            last_seen: ip.last_seen,
                        })
                    })
                    .collect();
                return Ok(ConnectionsView::Done { connections });
            }
            if attempt + 1 < CONNECTION_POLLS {
                tokio::time::sleep(CONNECTION_POLL_DELAY).await;
            }
        }
        Ok(ConnectionsView::Pending { job_id })
    }

    pub async fn subscription_requests(
        &self,
        id: SubscriptionId,
        actor: Actor,
    ) -> ServiceResult<Vec<RequestView>> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let mut records = remna.user_subscription_requests(user_id).await?.records;
        records.sort_by(|a, b| b.request_at.cmp(&a.request_at));
        Ok(records
            .into_iter()
            .take(REQUESTS_SHOWN)
            .map(|record| RequestView {
                at: record.request_at,
                response: record.srr_response_type,
                ip: record.request_ip,
                app: app_name(record.user_agent.as_deref()),
                user_agent: record.user_agent,
            })
            .collect())
    }

    pub async fn end_by_admin(&self, id: SubscriptionId, actor: Actor) -> ServiceResult<()> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        ensure_live(&subscription)?;
        self.services
            .renewals()
            .cancel_subscription(id, crate::renewals::CancelSource::Operator, actor)
            .await?;
        if let Some(link) = &subscription.remna {
            let remna = self.services.remna()?;
            let user_id = link.user_id;
            with_subscription_lock(self.services, id, async {
                match remna.delete_user(user_id).await {
                    Ok(()) => Ok(()),
                    Err(err) if err.is_not_found() => Ok(()),
                    Err(err) => Err(ServiceError::from(err)),
                }
            })
            .await?;
        }
        let (actor_kind, actor_id) = actor_fields(actor);
        let mut tx = self.services.store().begin().await?;
        tx.subscriptions().end(id, self.services.now()).await?;
        tx.audit()
            .record(&AuditEntry {
                actor_kind,
                actor_id,
                action: actions::DELETED.into(),
                entity_kind: Some("subscription".into()),
                entity_id: Some(id.0),
                details: json!({
                    "remna_user_id": subscription.remna.as_ref().map(|l| l.user_id),
                    "remna_username": subscription.remna.as_ref().map(|l| l.username.clone()),
                    "expire_at": subscription.mirror.expire_at,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        self.cancel_added_notice(&subscription).await;
        Ok(())
    }

    pub async fn set_autorenew(
        &self,
        id: SubscriptionId,
        enabled: bool,
        period_id: Option<PeriodId>,
        actor: Actor,
    ) -> ServiceResult<()> {
        let subscription = self.owned(id, actor).await?;
        let live = {
            let mut conn = self.services.store().conn().await?;
            crate::renewals::live_link(&mut conn, subscription.id).await?
        };
        if let Some(link) = live {
            if enabled {
                return Err(ServiceError::rejected(
                    "renewal.active",
                    format!(
                        "Подписку уже продлевает {}: сначала отключите это автопродление",
                        crate::renewals::title_of(self.services, &link.provider)
                    ),
                ));
            }
            let source = match actor {
                Actor::Admin(_) => crate::renewals::CancelSource::Operator,
                _ => crate::renewals::CancelSource::Client,
            };
            self.services
                .renewals()
                .cancel_link(link.id, source, actor)
                .await?;
        }
        let mut conn = self.services.store().conn().await?;
        conn.subscriptions()
            .set_autorenew(
                subscription.id,
                AutoRenewState {
                    enabled,
                    period_id: period_id.or(subscription.autorenew.period_id),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn add_days(
        &self,
        id: SubscriptionId,
        days: u32,
        actor: Actor,
    ) -> ServiceResult<SubscriptionView> {
        self.add_days_noted(id, days, actor, None).await
    }

    pub async fn add_days_noted(
        &self,
        id: SubscriptionId,
        days: u32,
        actor: Actor,
        note: Option<&serde_json::Value>,
    ) -> ServiceResult<SubscriptionView> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        if days == 0 || days > MAX_ADD_DAYS {
            return Err(ServiceError::rejected(
                "subscription.days",
                format!("можно добавить от 1 до {MAX_ADD_DAYS} дней"),
            ));
        }
        ensure_live(&subscription)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let after = with_subscription_lock(self.services, id, async {
            let before = self.fetch_user(&*remna, &subscription, user_id).await?;
            let after = match remna.extend_user(user_id, days).await {
                Ok(user) => user,
                Err(err) => {
                    confirm(&*remna, user_id, err, |u| u.expire_at != before.expire_at).await?
                }
            };
            self.record(Change {
                subscription: &subscription,
                actor,
                action: actions::ADD_DAYS,
                details: noted(
                    json!({
                        "days": days,
                        "previous_expire_at": before.expire_at,
                        "expire_at": after.expire_at,
                        "previous_status": status_of(&before),
                        "status": status_of(&after),
                    }),
                    note,
                ),
                user: Some(&after),
                devices: None,
                purchase: None,
            })
            .await?;
            Ok::<RemnaUser, ServiceError>(after)
        })
        .await?;
        if matches!(actor, Actor::Admin(_)) {
            self.notify_added(&subscription, after.expire_at).await;
        }
        self.view_of(id, Some(&after), Vec::new()).await
    }

    pub async fn undo_add_days(
        &self,
        id: SubscriptionId,
        count: u32,
        actor: Actor,
    ) -> ServiceResult<SubscriptionView> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        ensure_live(&subscription)?;
        if count == 0 || count > MAX_UNDO_STEPS {
            return Err(ServiceError::rejected(
                "subscription.undo_steps",
                format!("отменить можно от 1 до {MAX_UNDO_STEPS} добавлений"),
            ));
        }
        let user_id = linked_user(&subscription)?;
        let nothing = || {
            ServiceError::rejected(
                "subscription.nothing_to_undo",
                "нечего отменять: дни не добавлялись",
            )
        };
        let records: Vec<sn_store::AuditRecord> = {
            let mut conn = self.services.store().conn().await?;
            conn.audit()
                .for_entity("subscription", id.0, 200)
                .await?
                .into_iter()
                .filter(|r| r.action == actions::ADD_DAYS || r.action == actions::ADD_DAYS_UNDONE)
                .take(count as usize)
                .collect()
        };
        if records.len() < count as usize || records.iter().any(|r| r.action != actions::ADD_DAYS) {
            return Err(nothing());
        }
        let read = |record: &sn_store::AuditRecord, key: &str| {
            record
                .details
                .get(key)
                .cloned()
                .and_then(|v| serde_json::from_value::<DateTime<Utc>>(v).ok())
                .ok_or_else(nothing)
        };
        let changed = || {
            ServiceError::rejected(
                "subscription.changed_since",
                "срок уже менялся после добавления дней, отменить нельзя",
            )
        };
        for pair in records.windows(2) {
            if read(&pair[0], "previous_expire_at")? != read(&pair[1], "expire_at")? {
                return Err(changed());
            }
        }
        let (Some(newest), Some(oldest)) = (records.first(), records.last()) else {
            return Err(nothing());
        };
        let extended = read(newest, "expire_at")?;
        let previous = read(oldest, "previous_expire_at")?;
        let days: u64 = records
            .iter()
            .filter_map(|r| r.details.get("days").and_then(serde_json::Value::as_u64))
            .sum();
        let undone: Vec<i64> = records.iter().map(|r| r.id).collect();
        let remna = self.services.remna()?;
        let after = with_subscription_lock(self.services, id, async {
            let current = self.fetch_user(&*remna, &subscription, user_id).await?;
            if current.expire_at != extended {
                return Err(changed());
            }
            let mut request = UpdateUserRequest::new(user_id);
            request.expire_at = Some(previous);
            let after = match remna.update_user(&request).await {
                Ok(user) => user,
                Err(err) => confirm(&*remna, user_id, err, |u| u.expire_at == previous).await?,
            };
            self.record(Change {
                subscription: &subscription,
                actor,
                action: actions::ADD_DAYS_UNDONE,
                details: json!({
                    "undone": undone,
                    "days": days,
                    "previous_expire_at": extended,
                    "expire_at": after.expire_at,
                }),
                user: Some(&after),
                devices: None,
                purchase: None,
            })
            .await?;
            Ok(after)
        })
        .await?;
        self.cancel_added_notice(&subscription).await;
        self.view_of(id, Some(&after), Vec::new()).await
    }

    async fn notify_added(&self, subscription: &Subscription, expire_at: DateTime<Utc>) {
        let payload = JobPayload::Notify {
            client_id: subscription.client_id,
            template: RENEWED_TEMPLATE.into(),
            params: json!({
                "expire_at": expire_at,
                "subscription_id": subscription.id,
            }),
        };
        let run_at = self.services.now() + chrono::Duration::seconds(ADDED_NOTICE_DELAY_SECONDS);
        let queued = async {
            let mut conn = self.services.store().conn().await?;
            conn.jobs()
                .drop_queued_notifications(
                    subscription.client_id,
                    RENEWED_TEMPLATE,
                    subscription.id,
                )
                .await?;
            conn.jobs().enqueue(&payload, Some(run_at)).await?;
            Ok::<(), ServiceError>(())
        }
        .await;
        if let Err(err) = queued {
            tracing::warn!(subscription = %subscription.id, error = %err, "уведомление о добавленных днях не поставлено");
        }
    }

    async fn cancel_added_notice(&self, subscription: &Subscription) {
        let dropped = async {
            let mut conn = self.services.store().conn().await?;
            conn.jobs()
                .drop_queued_notifications(
                    subscription.client_id,
                    RENEWED_TEMPLATE,
                    subscription.id,
                )
                .await?;
            Ok::<(), ServiceError>(())
        }
        .await;
        if let Err(err) = dropped {
            tracing::warn!(subscription = %subscription.id, error = %err, "уведомление о добавленных днях не снято");
        }
    }

    pub async fn reset_traffic(
        &self,
        id: SubscriptionId,
        actor: Actor,
    ) -> ServiceResult<SubscriptionView> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        ensure_live(&subscription)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let before = self.fetch_user(&*remna, &subscription, user_id).await?;
        if before.traffic_limit_bytes <= 0 {
            return Err(ServiceError::rejected(
                "subscription.traffic_unlimited",
                "у подписки нет лимита трафика, сбрасывать нечего",
            ));
        }
        let used_before = before.user_traffic.used_traffic_bytes;
        let after = match remna.reset_user_traffic(user_id).await {
            Ok(user) => user,
            Err(err) => {
                confirm(&*remna, user_id, err, |u| {
                    u.last_traffic_reset_at != before.last_traffic_reset_at
                        || (used_before > 0 && u.user_traffic.used_traffic_bytes == 0)
                })
                .await?
            }
        };
        self.record(Change {
            subscription: &subscription,
            actor,
            action: actions::RESET_TRAFFIC,
            details: json!({
                "used_before": used_before,
                "limit": before.traffic_limit_bytes,
                "previous_status": status_of(&before),
                "status": status_of(&after),
            }),
            user: Some(&after),
            devices: None,
            purchase: None,
        })
        .await?;
        self.view_of(id, Some(&after), Vec::new()).await
    }

    fn client_may(&self, actor: Actor, allowed: bool, what: &str) -> ServiceResult<()> {
        if matches!(actor, Actor::Client(_)) && !allowed {
            return Err(ServiceError::rejected(
                "client.action_off",
                format!("{what} самостоятельно нельзя: напишите в поддержку"),
            ));
        }
        Ok(())
    }

    pub async fn reissue_link(&self, id: SubscriptionId, actor: Actor) -> ServiceResult<String> {
        let access = self.services.settings().client_access;
        self.client_may(actor, access.self_reissue, "Выпускать новую ссылку")?;
        let subscription = self.owned(id, actor).await?;
        ensure_live(&subscription)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let before = self.fetch_user(&*remna, &subscription, user_id).await?;
        let revoked = match remna.revoke_user(user_id, &RevokeRequest::default()).await {
            Ok(user) => user,
            Err(err) => {
                confirm(&*remna, user_id, err, |u| u.short_uuid != before.short_uuid).await?
            }
        };
        let cut = self.cut_devices(&*remna, user_id).await;
        let failed: Vec<&str> = cut.failed.iter().map(|(step, _)| *step).collect();
        self.record(Change {
            subscription: &subscription,
            actor,
            action: actions::REISSUE_LINK,
            details: json!({
                "previous_revoked_at": before.sub_revoked_at,
                "revoked_at": revoked.sub_revoked_at,
                "devices_removed": cut.remaining.is_some(),
                "failed_steps": failed,
            }),
            user: Some(&revoked),
            devices: cut.remaining,
            purchase: None,
        })
        .await?;
        if let Some((_, err)) = cut.failed.into_iter().next() {
            return Err(err);
        }
        Ok(revoked.subscription_url)
    }

    pub async fn disconnect_all_devices(
        &self,
        id: SubscriptionId,
        actor: Actor,
    ) -> ServiceResult<()> {
        let access = self.services.settings().client_access;
        self.client_may(actor, access.self_devices, "Отключать устройства")?;
        let subscription = self.owned(id, actor).await?;
        ensure_live(&subscription)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let before = self.fetch_user(&*remna, &subscription, user_id).await?;
        let request = RevokeRequest {
            revoke_only_passwords: Some(true),
            short_uuid: None,
        };
        let revoked = match remna.revoke_user(user_id, &request).await {
            Ok(user) => user,
            Err(err) => {
                confirm(&*remna, user_id, err, |u| {
                    u.sub_revoked_at != before.sub_revoked_at
                })
                .await?
            }
        };
        let cut = self.cut_devices(&*remna, user_id).await;
        let failed: Vec<&str> = cut.failed.iter().map(|(step, _)| *step).collect();
        self.record(Change {
            subscription: &subscription,
            actor,
            action: actions::DISCONNECT_ALL_DEVICES,
            details: json!({
                "devices_before": subscription.mirror.devices_count,
                "devices_removed": cut.remaining.is_some(),
                "failed_steps": failed,
            }),
            user: Some(&revoked),
            devices: cut.remaining,
            purchase: None,
        })
        .await?;
        if let Some((_, err)) = cut.failed.into_iter().next() {
            return Err(err);
        }
        Ok(())
    }

    pub async fn disconnect_device(
        &self,
        id: SubscriptionId,
        hwid: &str,
        actor: Actor,
    ) -> ServiceResult<()> {
        let access = self.services.settings().client_access;
        self.client_may(actor, access.self_devices, "Отключать устройства")?;
        let subscription = self.owned(id, actor).await?;
        ensure_live(&subscription)?;
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let listed = match remna.user_devices(user_id).await {
            Ok(devices) => devices,
            Err(err) if err.is_not_found() => {
                self.mark_missing(id).await?;
                return Err(ServiceError::NotFound("пользователь Remnawave"));
            }
            Err(err) => return Err(err.into()),
        };
        let device = listed
            .devices
            .iter()
            .find(|d| d.hwid == hwid)
            .cloned()
            .ok_or(ServiceError::NotFound("устройство"))?;
        let remaining = match remna.delete_user_device(user_id, hwid).await {
            Ok(devices) => devices,
            Err(err) => {
                confirm_devices(&*remna, user_id, err, |d| {
                    !d.devices.iter().any(|x| x.hwid == hwid)
                })
                .await?
            }
        };
        self.record(Change {
            subscription: &subscription,
            actor,
            action: actions::DISCONNECT_DEVICE,
            details: json!({
                "hwid": device.hwid,
                "title": device_title(&device),
                "platform": device.platform,
                "devices_left": devices_total(&remaining),
            }),
            user: None,
            devices: Some(devices_total(&remaining)),
            purchase: None,
        })
        .await
    }

    pub async fn devices(
        &self,
        id: SubscriptionId,
        actor: Actor,
    ) -> ServiceResult<Vec<DeviceView>> {
        let subscription = self.owned(id, actor).await?;
        let Some(link) = &subscription.remna else {
            return Ok(Vec::new());
        };
        let remna = self.services.remna()?;
        let devices = match remna.user_devices(link.user_id).await {
            Ok(devices) => devices,
            Err(err) if err.is_not_found() => {
                self.mark_missing(id).await?;
                return Err(ServiceError::NotFound("пользователь Remnawave"));
            }
            Err(err) => return Err(err.into()),
        };
        let mut conn = self.services.store().conn().await?;
        conn.subscriptions()
            .set_devices_count(id, devices_total(&devices))
            .await?;
        Ok(devices.devices.iter().map(device_view).collect())
    }

    pub async fn block_access(
        &self,
        client_id: ClientId,
        reason: Option<&str>,
    ) -> ServiceResult<()> {
        let subscriptions: Vec<Subscription> = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions()
                .by_client(client_id)
                .await?
                .into_iter()
                .filter(|s| s.ended_at.is_none())
                .collect()
        };
        let mut disabled: Vec<(SubscriptionId, RemnaUser)> = Vec::new();
        let mut missing: Vec<SubscriptionId> = Vec::new();
        let mut failed: Vec<SubscriptionId> = Vec::new();
        let mut errors: Vec<ServiceError> = Vec::new();
        let linked: Vec<(SubscriptionId, RemnaUserId)> = subscriptions
            .iter()
            .filter_map(|s| s.remna.as_ref().map(|link| (s.id, link.user_id)))
            .collect();
        if !linked.is_empty() {
            let remna = self.services.remna()?;
            for (id, user_id) in linked {
                let result = with_subscription_lock(self.services, id, async {
                    match disable_remna_user(&*remna, user_id).await {
                        Ok(user) => Ok(Some(user)),
                        Err(err) if err.is_not_found() => Ok(None),
                        Err(err) => {
                            confirm(&*remna, user_id, err, |u| u.status == UserStatus::Disabled)
                                .await
                                .map(Some)
                        }
                    }
                })
                .await;
                match result {
                    Ok(Some(user)) => disabled.push((id, user)),
                    Ok(None) => missing.push(id),
                    Err(err) => {
                        failed.push(id);
                        errors.push(err);
                    }
                }
            }
            if !disabled.is_empty() {
                let ids = disabled.iter().map(|(_, user)| user.id).collect();
                if let Err(err) = drop_everywhere(&*remna, ids).await {
                    errors.push(err.into());
                }
            }
        }
        let paused: Vec<&Subscription> = subscriptions
            .iter()
            .filter(|s| s.autorenew.enabled)
            .collect();
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        for (id, user) in &disabled {
            store_user(&mut tx, *id, user, None, now).await?;
        }
        for id in &missing {
            tx.subscriptions().mark_missing(*id, true).await?;
        }
        for subscription in &paused {
            tx.subscriptions()
                .set_autorenew(
                    subscription.id,
                    AutoRenewState {
                        enabled: false,
                        period_id: subscription.autorenew.period_id,
                    },
                )
                .await?;
        }
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: actions::BLOCK_ACCESS.to_string(),
                entity_kind: Some("client".into()),
                entity_id: Some(client_id.0),
                details: json!({
                    "reason": reason,
                    "disabled": disabled.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                    "missing": missing,
                    "failed": failed,
                    "autorenew_paused": paused.iter().map(|s| s.id).collect::<Vec<_>>(),
                    "at": now,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        match errors.into_iter().next() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    async fn paused_autorenew(
        &self,
        client_id: ClientId,
    ) -> ServiceResult<BTreeSet<SubscriptionId>> {
        let mut conn = self.services.store().conn().await?;
        let records = conn.audit().for_entity("client", client_id.0, 100).await?;
        let mut paused = BTreeSet::new();
        for record in records {
            if record.action == actions::RESTORE_ACCESS {
                break;
            }
            if record.action != actions::BLOCK_ACCESS {
                continue;
            }
            let ids = record
                .details
                .get("autorenew_paused")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_i64)
                .map(SubscriptionId);
            paused.extend(ids);
        }
        Ok(paused)
    }

    pub async fn restore_access(&self, client_id: ClientId) -> ServiceResult<()> {
        let subscriptions: Vec<Subscription> = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions()
                .by_client(client_id)
                .await?
                .into_iter()
                .filter(|s| s.ended_at.is_none())
                .collect()
        };
        let now = self.services.now();
        let mut refreshed: Vec<(SubscriptionId, RemnaUser)> = Vec::new();
        let mut enabled: Vec<SubscriptionId> = Vec::new();
        let mut working: Vec<SubscriptionId> = Vec::new();
        let mut expired: Vec<SubscriptionId> = Vec::new();
        let mut missing: Vec<SubscriptionId> = Vec::new();
        let mut failed: Vec<SubscriptionId> = Vec::new();
        let mut errors: Vec<ServiceError> = Vec::new();
        let linked: Vec<(SubscriptionId, RemnaUserId)> = subscriptions
            .iter()
            .filter_map(|s| s.remna.as_ref().map(|link| (s.id, link.user_id)))
            .collect();
        if !linked.is_empty() {
            let remna = self.services.remna()?;
            for (id, user_id) in linked {
                let result = with_subscription_lock(self.services, id, async {
                    let current = match remna.get_user(user_id).await {
                        Ok(user) => user,
                        Err(err) if err.is_not_found() => return Ok(Restored::Missing),
                        Err(err) => return Err(ServiceError::from(err)),
                    };
                    if current.status != UserStatus::Disabled {
                        return Ok(Restored::Untouched(current));
                    }
                    match enable_remna_user(&*remna, user_id).await {
                        Ok(user) => Ok(Restored::Enabled(user)),
                        Err(err) => {
                            confirm(&*remna, user_id, err, |u| u.status != UserStatus::Disabled)
                                .await
                                .map(Restored::Enabled)
                        }
                    }
                })
                .await;
                let user = match result {
                    Ok(Restored::Missing) => {
                        missing.push(id);
                        continue;
                    }
                    Ok(Restored::Untouched(user)) => user,
                    Ok(Restored::Enabled(user)) => {
                        enabled.push(id);
                        user
                    }
                    Err(err) => {
                        failed.push(id);
                        errors.push(err);
                        continue;
                    }
                };
                if user.expire_at <= now {
                    expired.push(id);
                } else {
                    working.push(id);
                }
                refreshed.push((id, user));
            }
        }
        let paused = self.paused_autorenew(client_id).await?;
        let resumed: Vec<&Subscription> = subscriptions
            .iter()
            .filter(|s| paused.contains(&s.id) && working.contains(&s.id) && !s.autorenew.enabled)
            .collect();
        let mut tx = self.services.store().begin().await?;
        for (id, user) in &refreshed {
            store_user(&mut tx, *id, user, None, now).await?;
        }
        for id in &missing {
            tx.subscriptions().mark_missing(*id, true).await?;
        }
        for subscription in &resumed {
            tx.subscriptions()
                .set_autorenew(
                    subscription.id,
                    AutoRenewState {
                        enabled: true,
                        period_id: subscription.autorenew.period_id,
                    },
                )
                .await?;
        }
        tx.audit()
            .record(&AuditEntry {
                actor_kind: ActorKind::System,
                actor_id: None,
                action: actions::RESTORE_ACCESS.to_string(),
                entity_kind: Some("client".into()),
                entity_id: Some(client_id.0),
                details: json!({
                    "enabled": enabled,
                    "expired": expired,
                    "missing": missing,
                    "failed": failed,
                    "autorenew_resumed": resumed.iter().map(|s| s.id).collect::<Vec<_>>(),
                    "at": now,
                }),
                ip: None,
            })
            .await?;
        tx.commit().await?;
        match errors.into_iter().next() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    pub async fn change_limits(
        &self,
        id: SubscriptionId,
        change: &LimitsChange,
        actor: Actor,
    ) -> ServiceResult<SubscriptionView> {
        self.change_limits_noted(id, change, actor, None).await
    }

    pub async fn change_limits_noted(
        &self,
        id: SubscriptionId,
        change: &LimitsChange,
        actor: Actor,
        note: Option<&serde_json::Value>,
    ) -> ServiceResult<SubscriptionView> {
        let subscription = self.owned(id, actor).await?;
        staff_only(actor)?;
        ensure_live(&subscription)?;
        if change.traffic_limit_bytes.is_none()
            && change.extra_devices.is_none()
            && change.devices.is_none()
        {
            return self.view_of(id, None, Vec::new()).await;
        }
        if change.devices.is_some() && change.extra_devices.is_some() {
            return Err(ServiceError::rejected(
                "subscription.devices_twice",
                "укажите либо число устройств всего, либо доп. устройства",
            ));
        }
        if let Some(total) = change.devices
            && !(1..=MAX_DEVICES).contains(&total)
        {
            return Err(ServiceError::rejected(
                "subscription.devices_range",
                format!("устройств может быть от 1 до {MAX_DEVICES}"),
            ));
        }
        let traffic_limit = match change.traffic_limit_bytes {
            Some(Some(0)) => {
                return Err(ServiceError::rejected(
                    "subscription.traffic_zero",
                    "лимит 0 байт не имеет смысла: оставьте поле пустым для безлимита",
                ));
            }
            Some(Some(bytes)) => Some(i64::try_from(bytes).map_err(|_| {
                ServiceError::rejected("subscription.traffic_too_big", "слишком большой лимит")
            })?),
            Some(None) => Some(0),
            None => None,
        };
        let policy = match subscription.tariff_id {
            Some(tariff_id) => {
                let mut conn = self.services.store().conn().await?;
                match conn.tariffs().get(tariff_id).await {
                    Ok(tariff) => Some(tariff.devices),
                    Err(err) if err.is_not_found() => None,
                    Err(err) => return Err(err.into()),
                }
            }
            None => None,
        };
        let included = policy.as_ref().map(|p| p.included);
        let (extra_target, manual_target) = match change.devices {
            Some(total) => match &policy {
                Some(p)
                    if total >= p.included
                        && total <= p.included + p.extra.as_ref().map_or(0, |e| e.max) =>
                {
                    (Some(total - p.included), Some(None))
                }
                _ => (None, Some(Some(total))),
            },
            None => (
                change.extra_devices,
                change.extra_devices.map(|_| None::<u32>),
            ),
        };
        let (purchase, bare_extra) = match extra_target {
            Some(extra) if extra != subscription.extra_devices => {
                match (subscription.period_started_at, subscription.period_days) {
                    (Some(period_started_at), Some(period_days)) => (
                        Some(PurchaseApplied {
                            tariff_id: subscription.tariff_id,
                            kind: subscription.kind,
                            extra_devices: extra,
                            period_started_at,
                            period_days,
                        }),
                        None,
                    ),
                    _ => (None, Some(extra)),
                }
            }
            _ => (None, None),
        };
        let user_id = linked_user(&subscription)?;
        let remna = self.services.remna()?;
        let (after, errors) = with_subscription_lock(self.services, id, async {
            let before = self.fetch_user(&*remna, &subscription, user_id).await?;
            let target_limit = traffic_limit.unwrap_or(before.traffic_limit_bytes);
            let target_hwid: Option<Option<i64>> = match manual_target {
                Some(Some(total)) => Some(Some(i64::from(total))),
                _ => extra_target.map(|extra| {
                    let base = included.map(i64::from).unwrap_or_else(|| {
                        (before.hwid_device_limit.unwrap_or(0)
                            - i64::from(subscription.extra_devices))
                        .max(0)
                    });
                    let total = base + i64::from(extra);
                    (total > 0).then_some(total)
                }),
            };
            let mut request = UpdateUserRequest::new(user_id);
            request.traffic_limit_bytes = traffic_limit;
            request.hwid_device_limit = target_hwid;
            let traffic_left =
                target_limit == 0 || target_limit > before.user_traffic.used_traffic_bytes;
            if before.status == UserStatus::Limited && traffic_limit.is_some() && traffic_left {
                request.status = Some(UserStatus::Active);
            }
            let after = match remna.update_user(&request).await {
                Ok(user) => user,
                Err(err) => {
                    confirm(&*remna, user_id, err, |u| {
                        u.traffic_limit_bytes == target_limit
                            && target_hwid.is_none_or(|hwid| u.hwid_device_limit == hwid)
                    })
                    .await?
                }
            };
            let trimmed = match target_hwid {
                Some(Some(limit)) => trim_devices(&*remna, user_id, limit).await,
                _ => Trimmed {
                    removed: Vec::new(),
                    left: None,
                    errors: Vec::new(),
                },
            };
            if let Some(manual) = manual_target {
                let mut conn = self.services.store().conn().await?;
                sqlx::query("DELETE FROM subscription_device_holds WHERE subscription_id = $1")
                    .bind(id.0)
                    .execute(conn.conn())
                    .await?;
                if let Some(extra) = bare_extra {
                    conn.subscriptions().set_extra_devices(id, extra).await?;
                }
                conn.subscriptions().set_device_limit(id, manual).await?;
            }
            if traffic_limit.is_some() {
                let mut conn = self.services.store().conn().await?;
                conn.subscriptions().clear_extra_traffic(id).await?;
            }
            self.record(Change {
                subscription: &subscription,
                actor,
                action: actions::CHANGE_LIMITS,
                details: noted(
                    json!({
                        "traffic_limit_before": before.traffic_limit_bytes,
                        "traffic_limit": after.traffic_limit_bytes,
                        "hwid_limit_before": before.hwid_device_limit,
                        "hwid_limit": after.hwid_device_limit,
                        "extra_devices_before": subscription.extra_devices,
                        "extra_devices": extra_target.unwrap_or(subscription.extra_devices),
                        "device_limit_before": subscription.device_limit,
                        "device_limit": manual_target.unwrap_or(subscription.device_limit),
                        "removed_devices": trimmed.removed,
                        "previous_status": status_of(&before),
                        "status": status_of(&after),
                    }),
                    note,
                ),
                user: Some(&after),
                devices: trimmed.left,
                purchase,
            })
            .await?;
            Ok::<_, ServiceError>((after, trimmed.errors))
        })
        .await?;
        if let Some(err) = errors.into_iter().next() {
            return Err(err);
        }
        self.view_of(id, Some(&after), Vec::new()).await
    }

    pub async fn release_held_devices(&self, now: DateTime<Utc>) -> ServiceResult<usize> {
        let due: Vec<i64> = {
            let mut conn = self.services.store().conn().await?;
            sqlx::query_scalar(
                "SELECT subscription_id FROM subscription_device_holds WHERE held_until <= $1 \
                 ORDER BY held_until LIMIT $2",
            )
            .bind(now)
            .bind(HOLDS_BATCH)
            .fetch_all(conn.conn())
            .await?
        };
        let mut released = 0;
        for id in due.into_iter().map(SubscriptionId) {
            match with_subscription_lock(self.services, id, self.release_hold(id, now)).await {
                Ok(true) => released += 1,
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(subscription = %id, error = %err, "оплаченные доп. устройства не сняты");
                }
            }
        }
        Ok(released)
    }

    async fn release_hold(&self, id: SubscriptionId, now: DateTime<Utc>) -> ServiceResult<bool> {
        let (hold, subscription) = {
            let mut conn = self.services.store().conn().await?;
            let hold: Option<(i32, DateTime<Utc>)> = sqlx::query_as(
                "SELECT limit_after, held_until FROM subscription_device_holds \
                 WHERE subscription_id = $1",
            )
            .bind(id.0)
            .fetch_optional(conn.conn())
            .await?;
            (hold, conn.subscriptions().get(id).await?)
        };
        let Some((limit_after, held_until)) = hold.filter(|(_, until)| *until <= now) else {
            return Ok(false);
        };
        if let (Some(link), None) = (&subscription.remna, subscription.ended_at) {
            let remna = self.services.remna()?;
            match remna.get_user(link.user_id).await {
                Ok(before) => {
                    self.reduce_devices(&*remna, &subscription, &before, limit_after, held_until)
                        .await?;
                }
                Err(err) if err.is_not_found() => self.mark_missing(id).await?,
                Err(err) => return Err(err.into()),
            }
        }
        let mut conn = self.services.store().conn().await?;
        sqlx::query(
            "DELETE FROM subscription_device_holds WHERE subscription_id = $1 AND held_until <= $2",
        )
        .bind(id.0)
        .bind(now)
        .execute(conn.conn())
        .await?;
        Ok(true)
    }

    async fn reduce_devices(
        &self,
        remna: &dyn RemnawaveApi,
        subscription: &Subscription,
        before: &RemnaUser,
        limit_after: i32,
        held_until: DateTime<Utc>,
    ) -> ServiceResult<()> {
        let Some(current) = before.hwid_device_limit else {
            return Ok(());
        };
        let limit = i64::from(limit_after);
        let after = if current > limit {
            let mut request = UpdateUserRequest::new(before.id);
            request.hwid_device_limit = Some(Some(limit));
            Some(match remna.update_user(&request).await {
                Ok(user) => user,
                Err(err) => {
                    confirm(remna, before.id, err, |u| {
                        u.hwid_device_limit == Some(limit)
                    })
                    .await?
                }
            })
        } else {
            None
        };
        let trimmed = trim_devices(remna, before.id, current.min(limit)).await;
        if after.is_some() || !trimmed.removed.is_empty() {
            let user = after.as_ref().unwrap_or(before);
            self.record(Change {
                subscription,
                actor: Actor::System,
                action: actions::RELEASE_DEVICES,
                details: json!({
                    "held_until": held_until,
                    "hwid_limit_before": before.hwid_device_limit,
                    "hwid_limit": user.hwid_device_limit,
                    "removed_devices": trimmed.removed,
                }),
                user: Some(user),
                devices: trimmed.left,
                purchase: None,
            })
            .await?;
        }
        match trimmed.errors.into_iter().next() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{EnvFallbacks, ServiceDeps, SystemClock};
    use chrono::Duration;
    use sn_domain::client::{Client, Locale, RegistrationSource};
    use sn_domain::events::DomainEvent;
    use sn_domain::ids::AdminId;
    use sn_domain::subscription::{RemnaLink, SubscriptionKind};
    use sn_domain::tariff::{CurrencyPrices, DevicePolicy, ExtraDevices};
    use sn_domain::traffic::{GIB, TrafficPolicy, TrafficStrategy};
    use sn_modules::ModuleRegistry;
    use sn_payments::ProviderRegistry;
    use sn_remnawave::fake::{
        FakeRemnawave, Fault, panel_time, sample_device, sample_node, sample_user,
    };
    use sn_store::testing::{TestDb, test_store};
    use sn_store::{NewCategory, NewClient, NewPeriod, NewSubscription, NewTariff};
    use std::sync::Arc;
    use uuid::Uuid;

    pub(crate) const ADMIN: Actor = Actor::Admin(AdminId(1));

    pub(crate) struct Env {
        pub(crate) db: TestDb,
        pub(crate) services: Services,
        pub(crate) fake: Arc<FakeRemnawave>,
    }

    impl Env {
        pub(crate) async fn finish(self) {
            self.db.cleanup().await;
        }

        pub(crate) async fn client(&self) -> Client {
            let mut conn = self.db.store.conn().await.unwrap();
            conn.clients()
                .create(&NewClient {
                    locale: Locale::Ru,
                    registration_source: RegistrationSource::Bot,
                    note: None,
                })
                .await
                .unwrap()
        }

        pub(crate) async fn linked(&self, client: ClientId, user: RemnaUser) -> Subscription {
            self.fake.insert_user(user.clone());
            let devices = u32::try_from(self.fake.devices(user.id).len()).unwrap();
            let mut tx = self.db.store.begin().await.unwrap();
            let subscription = tx
                .subscriptions()
                .create(&NewSubscription {
                    client_id: client,
                    tariff_id: None,
                    kind: SubscriptionKind::Paid,
                    extra_devices: 0,
                    autorenew: AutoRenewState::default(),
                })
                .await
                .unwrap();
            tx.subscriptions()
                .link_remna(
                    subscription.id,
                    &RemnaLink {
                        user_id: user.id,
                        username: user.username.clone(),
                        short_uuid: Some(user.short_uuid.clone()),
                    },
                )
                .await
                .unwrap();
            tx.subscriptions()
                .apply_mirror(
                    subscription.id,
                    &user.mirror(Some(devices), Utc::now() - Duration::hours(1)),
                )
                .await
                .unwrap();
            tx.commit().await.unwrap();
            self.subscription(subscription.id).await
        }

        pub(crate) async fn subscription(&self, id: SubscriptionId) -> Subscription {
            let mut conn = self.db.store.conn().await.unwrap();
            conn.subscriptions().get(id).await.unwrap()
        }

        pub(crate) async fn status_events(
            &self,
            client: ClientId,
        ) -> Vec<(Option<SubscriptionStatus>, SubscriptionStatus)> {
            let mut conn = self.db.store.conn().await.unwrap();
            let mut events: Vec<_> = conn
                .events()
                .for_client(client, 50)
                .await
                .unwrap()
                .into_iter()
                .filter_map(|stored| match stored.event {
                    DomainEvent::SubscriptionStatusChanged { from, to, .. } => Some((from, to)),
                    _ => None,
                })
                .collect();
            events.reverse();
            events
        }

        pub(crate) async fn audit(&self, kind: &str, id: i64) -> Vec<sn_store::AuditRecord> {
            let mut conn = self.db.store.conn().await.unwrap();
            conn.audit().for_entity(kind, id, 20).await.unwrap()
        }
    }

    pub(crate) async fn env() -> Option<Env> {
        let db = test_store().await?;
        let services = Services::new(ServiceDeps {
            store: db.store.clone(),
            payments: Arc::new(ProviderRegistry::builtin()),
            modules: Arc::new(ModuleRegistry::catalog_only()),
            secrets: None,
            clock: Arc::new(SystemClock),
            env: EnvFallbacks::default(),
        });
        let fake = Arc::new(FakeRemnawave::new());
        let api: Arc<dyn RemnawaveApi> = fake.clone();
        services.set_remna(Some(api));
        Some(Env { db, services, fake })
    }

    fn code(err: &ServiceError) -> &'static str {
        err.code()
    }

    #[test]
    fn имя_устройства_и_приложение_из_данных_remnawave() {
        let mut device = sample_device(RemnaUserId(1), "hw", Utc::now());
        assert_eq!(device_title(&device), "iPhone 15 Pro");
        device.device_model = Some("iPhone16,1".into());
        assert_eq!(device_title(&device), "iPhone");
        device.device_model = Some("  ".into());
        device.platform = Some("Android".into());
        assert_eq!(device_title(&device), "Android");
        device.device_model = Some("MacBookPro18,3".into());
        device.platform = Some("macOS".into());
        assert_eq!(device_title(&device), "Mac");
        device.device_model = None;
        device.platform = Some("Tizen".into());
        assert_eq!(device_title(&device), "Tizen");
        device.platform = None;
        assert_eq!(device_title(&device), "Устройство");
        assert_eq!(app_name(Some("Happ/3.2.1")).as_deref(), Some("Happ 3.2"));
        assert_eq!(
            app_name(Some("Happ/3.2/ios CFNetwork")).as_deref(),
            Some("Happ 3.2")
        );
        assert_eq!(
            app_name(Some("clash-verge/v2.0.3")).as_deref(),
            Some("clash-verge 2.0")
        );
        assert_eq!(app_name(Some("Hiddify")).as_deref(), Some("Hiddify"));
        assert_eq!(app_name(Some("v2rayTun/5")).as_deref(), Some("v2rayTun 5"));
        assert_eq!(app_name(Some("Mozilla/5.0 (iPhone)")), None);
        assert_eq!(app_name(Some("")), None);
        assert_eq!(app_name(None), None);
        let view = device_view(&sample_device(RemnaUserId(1), "hw-1", Utc::now()));
        assert_eq!(view.app.as_deref(), Some("Happ 3.2"));
        assert_eq!(view.os_version.as_deref(), Some("18.1"));
    }

    #[tokio::test]
    async fn добавление_дней_пишет_прежний_срок_и_отменяется() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let user = sample_user(1, "sn_add");
        let before = user.expire_at;
        let subscription = env.linked(client.id, user).await;
        let view = env
            .services
            .subscriptions()
            .add_days(subscription.id, 7, ADMIN)
            .await
            .unwrap();
        let after = env.fake.user(RemnaUserId(1)).unwrap().expire_at;
        assert_eq!(after, before + Duration::days(7));
        assert_eq!(
            view.subscription
                .mirror
                .expire_at
                .map(|t| t.timestamp_micros()),
            Some(after.timestamp_micros())
        );
        assert_eq!(
            view.subscription_url.as_deref(),
            Some("https://sub.example.com/short1")
        );
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].action, actions::ADD_DAYS);
        assert_eq!(audit[0].actor_kind, "admin");
        assert_eq!(audit[0].details["days"], 7);
        let previous: DateTime<Utc> =
            serde_json::from_value(audit[0].details["previous_expire_at"].clone()).unwrap();
        assert_eq!(previous, before);
        assert!(env.status_events(client.id).await.is_empty());

        env.services
            .subscriptions()
            .undo_add_days(subscription.id, 1, ADMIN)
            .await
            .unwrap();
        assert_eq!(env.fake.user(RemnaUserId(1)).unwrap().expire_at, before);
        let err = env
            .services
            .subscriptions()
            .undo_add_days(subscription.id, 1, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.nothing_to_undo");
        let err = env
            .services
            .subscriptions()
            .add_days(subscription.id, 0, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.days");
        env.finish().await;
    }

    #[tokio::test]
    async fn несколько_добавлений_отменяются_разом_и_уведомление_снимается() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_undo_many")).await;
        let before = env.fake.user(RemnaUserId(1)).unwrap().expire_at;
        let ops = env.services.subscriptions();
        ops.add_days(subscription.id, 7, ADMIN).await.unwrap();
        let view = ops.add_days(subscription.id, 30, ADMIN).await.unwrap();
        let queued = || async {
            let mut conn = env.services.store().conn().await.unwrap();
            sqlx::query_as::<_, (serde_json::Value, DateTime<Utc>)>(
                "SELECT payload, run_at FROM jobs WHERE kind = 'notify' AND status = 'queued'",
            )
            .fetch_all(conn.conn())
            .await
            .unwrap()
        };
        let notices = queued().await;
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].0["template"], RENEWED_TEMPLATE);
        assert_eq!(notices[0].0["params"]["subscription_id"], subscription.id.0);
        let promised: DateTime<Utc> =
            serde_json::from_value(notices[0].0["params"]["expire_at"].clone()).unwrap();
        assert_eq!(Some(promised), view.subscription.mirror.expire_at);
        assert!(notices[0].1 > Utc::now() + Duration::seconds(30));

        let err = ops
            .undo_add_days(subscription.id, 3, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.nothing_to_undo");
        let err = ops
            .undo_add_days(subscription.id, 0, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.undo_steps");
        ops.undo_add_days(subscription.id, 2, ADMIN).await.unwrap();
        assert_eq!(env.fake.user(RemnaUserId(1)).unwrap().expire_at, before);
        assert!(queued().await.is_empty());
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].action, actions::ADD_DAYS_UNDONE);
        assert_eq!(audit[0].details["days"], 37);
        assert_eq!(audit[0].details["undone"].as_array().map(Vec::len), Some(2));
        let err = ops
            .undo_add_days(subscription.id, 1, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.nothing_to_undo");
        env.finish().await;
    }

    #[tokio::test]
    async fn отмена_невозможна_если_срок_менялся_после() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_undo")).await;
        let ops = env.services.subscriptions();
        ops.add_days(subscription.id, 7, ADMIN).await.unwrap();
        let mut patch = UpdateUserRequest::new(RemnaUserId(1));
        patch.expire_at = Some(Utc::now() + Duration::days(400));
        env.fake.update_user(&patch).await.unwrap();
        let err = ops
            .undo_add_days(subscription.id, 1, ADMIN)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.changed_since");
        env.finish().await;
    }

    #[tokio::test]
    async fn продление_истёкшей_подписки_меняет_статус_и_шлёт_событие() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let mut user = sample_user(1, "sn_expired");
        user.status = UserStatus::Expired;
        user.expire_at = Utc::now() - Duration::days(5);
        let subscription = env.linked(client.id, user).await;
        let started = panel_time(Utc::now());
        let view = env
            .services
            .subscriptions()
            .add_days(subscription.id, 30, Actor::System)
            .await
            .unwrap();
        let fresh = env.fake.user(RemnaUserId(1)).unwrap();
        assert_eq!(fresh.status, UserStatus::Active);
        assert!(fresh.expire_at >= started + Duration::days(30));
        assert_eq!(
            view.subscription.mirror.status,
            Some(SubscriptionStatus::Active)
        );
        assert_eq!(
            env.status_events(client.id).await,
            vec![(
                Some(SubscriptionStatus::Expired),
                SubscriptionStatus::Active
            )]
        );
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].actor_kind, "system");
        assert_eq!(audit[0].details["previous_status"], "expired");
        env.finish().await;
    }

    #[tokio::test]
    async fn таймаут_после_продления_не_продлевает_дважды() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let user = sample_user(1, "sn_timeout");
        let before = user.expire_at;
        let subscription = env.linked(client.id, user).await;
        let ops = env.services.subscriptions();
        env.fake.fail_on("extend_user", Fault::TimeoutAfterApply);
        ops.add_days(subscription.id, 7, ADMIN).await.unwrap();
        assert_eq!(
            env.fake.user(RemnaUserId(1)).unwrap().expire_at,
            before + Duration::days(7)
        );
        assert_eq!(env.fake.calls_of("extend_user").len(), 1);

        env.fake.fail_on("extend_user", Fault::Timeout);
        let err = ops.add_days(subscription.id, 7, ADMIN).await.unwrap_err();
        assert_eq!(code(&err), "remnawave.unavailable");
        assert_eq!(
            env.fake.user(RemnaUserId(1)).unwrap().expire_at,
            before + Duration::days(7)
        );
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit.len(), 1);
        env.finish().await;
    }

    #[tokio::test]
    async fn чужая_подписка_не_видна_а_дни_клиент_себе_не_добавит() {
        let Some(env) = env().await else { return };
        let owner = env.client().await;
        let stranger = env.client().await;
        let subscription = env.linked(owner.id, sample_user(1, "sn_owner")).await;
        let ops = env.services.subscriptions();
        let other = Actor::Client(stranger.id);
        let id = subscription.id;
        assert_eq!(
            code(&ops.view(id, other, true).await.unwrap_err()),
            "not_found"
        );
        assert_eq!(
            code(&ops.add_days(id, 7, other).await.unwrap_err()),
            "not_found"
        );
        assert_eq!(
            code(&ops.reset_traffic(id, other).await.unwrap_err()),
            "not_found"
        );
        assert_eq!(
            code(&ops.reissue_link(id, other).await.unwrap_err()),
            "not_found"
        );
        assert_eq!(
            code(&ops.disconnect_all_devices(id, other).await.unwrap_err()),
            "not_found"
        );
        assert_eq!(
            code(&ops.disconnect_device(id, "hw", other).await.unwrap_err()),
            "not_found"
        );
        assert_eq!(
            code(&ops.devices(id, other).await.unwrap_err()),
            "not_found"
        );
        let limits = LimitsChange {
            traffic_limit_bytes: Some(None),
            extra_devices: None,
            devices: None,
        };
        assert_eq!(
            code(&ops.change_limits(id, &limits, other).await.unwrap_err()),
            "not_found"
        );
        let me = Actor::Client(owner.id);
        assert_eq!(
            code(&ops.add_days(id, 7, me).await.unwrap_err()),
            "auth.forbidden"
        );
        assert_eq!(
            code(&ops.reset_traffic(id, me).await.unwrap_err()),
            "auth.forbidden"
        );
        assert_eq!(
            code(&ops.change_limits(id, &limits, me).await.unwrap_err()),
            "auth.forbidden"
        );
        assert!(ops.view(id, me, true).await.is_ok());
        assert!(!env.fake.calls().iter().any(|c| c == "extend_user"));
        env.finish().await;
    }

    #[tokio::test]
    async fn сброс_трафика_только_при_лимите_и_снимает_limited() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let ops = env.services.subscriptions();
        let unlimited = env.linked(client.id, sample_user(1, "sn_unlimited")).await;
        let err = ops.reset_traffic(unlimited.id, ADMIN).await.unwrap_err();
        assert_eq!(code(&err), "subscription.traffic_unlimited");
        assert!(!env.fake.calls().iter().any(|c| c == "reset_user_traffic"));

        let mut user = sample_user(2, "sn_limited");
        user.status = UserStatus::Limited;
        user.traffic_limit_bytes = (300 * GIB) as i64;
        user.traffic_limit_strategy = TrafficStrategy::Month;
        user.user_traffic.used_traffic_bytes = (300 * GIB) as i64;
        let limited = env.linked(client.id, user).await;
        env.fake
            .fail_on("reset_user_traffic", Fault::TimeoutAfterApply);
        let view = ops.reset_traffic(limited.id, ADMIN).await.unwrap();
        assert_eq!(view.subscription.mirror.traffic_used_bytes, Some(0));
        assert_eq!(
            view.subscription.mirror.status,
            Some(SubscriptionStatus::Active)
        );
        assert_eq!(env.fake.calls_of("reset_user_traffic").len(), 1);
        assert_eq!(
            env.status_events(client.id).await,
            vec![(
                Some(SubscriptionStatus::Limited),
                SubscriptionStatus::Active
            )]
        );
        let audit = env.audit("subscription", limited.id.0).await;
        assert_eq!(audit[0].action, actions::RESET_TRAFFIC);
        assert_eq!(audit[0].details["used_before"], (300 * GIB) as i64);
        env.finish().await;
    }

    #[tokio::test]
    async fn новая_ссылка_меняет_адрес_удаляет_устройства_и_рвёт_подключения() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let user = sample_user(1, "sn_link");
        let old_url = user.subscription_url.clone();
        env.fake
            .add_device(sample_device(RemnaUserId(1), "hw-1", Utc::now()));
        env.fake
            .add_device(sample_device(RemnaUserId(1), "hw-2", Utc::now()));
        let subscription = env.linked(client.id, user).await;
        assert_eq!(subscription.mirror.devices_count, Some(2));
        env.fake.fail_on("revoke_user", Fault::TimeoutAfterApply);
        let url = env
            .services
            .subscriptions()
            .reissue_link(subscription.id, Actor::Client(client.id))
            .await
            .unwrap();
        assert_ne!(url, old_url);
        let fresh = env.fake.user(RemnaUserId(1)).unwrap();
        assert_eq!(url, fresh.subscription_url);
        assert_eq!(env.fake.calls_of("revoke_user").len(), 1);
        assert_eq!(env.fake.calls_of("revoke_user")[0]["request"], json!({}));
        assert!(env.fake.devices(RemnaUserId(1)).is_empty());
        assert_eq!(
            env.fake.calls_of("drop_connections"),
            vec![
                json!({"dropBy": {"by": "userIds", "userIds": [1]}, "targetNodes": {"target": "allNodes"}})
            ]
        );
        let stored = env.subscription(subscription.id).await;
        assert_eq!(
            stored.remna.unwrap().short_uuid.as_deref(),
            Some(fresh.short_uuid.as_str())
        );
        assert_eq!(stored.mirror.devices_count, Some(0));
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].action, actions::REISSUE_LINK);
        assert_eq!(audit[0].actor_kind, "client");
        assert!(!audit[0].details.to_string().contains(&fresh.short_uuid));
        env.finish().await;
    }

    #[tokio::test]
    async fn отключение_всех_устройств_оставляет_ссылку() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let user = sample_user(1, "sn_devices");
        let old_url = user.subscription_url.clone();
        env.fake
            .add_device(sample_device(RemnaUserId(1), "hw-1", Utc::now()));
        let subscription = env.linked(client.id, user).await;
        env.fake
            .fail_on("delete_all_user_devices", Fault::TimeoutAfterApply);
        env.services
            .subscriptions()
            .disconnect_all_devices(subscription.id, ADMIN)
            .await
            .unwrap();
        let fresh = env.fake.user(RemnaUserId(1)).unwrap();
        assert_eq!(fresh.subscription_url, old_url);
        assert!(fresh.sub_revoked_at.is_some());
        assert_eq!(
            env.fake.calls_of("revoke_user")[0]["request"],
            json!({"revokeOnlyPasswords": true})
        );
        assert!(env.fake.devices(RemnaUserId(1)).is_empty());
        assert_eq!(env.fake.calls_of("drop_connections").len(), 1);
        assert_eq!(
            env.subscription(subscription.id).await.mirror.devices_count,
            Some(0)
        );
        env.finish().await;
    }

    #[tokio::test]
    async fn отключение_одного_устройства_и_список_устройств() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let now = Utc::now();
        let mut coded = sample_device(RemnaUserId(1), "hw-1", now - Duration::days(30));
        coded.device_model = Some("iPhone16,1".into());
        env.fake.add_device(coded);
        env.fake
            .add_device(sample_device(RemnaUserId(1), "hw-2", now));
        let subscription = env.linked(client.id, sample_user(1, "sn_one")).await;
        let ops = env.services.subscriptions();
        let owner = Actor::Client(client.id);
        let devices = ops.devices(subscription.id, owner).await.unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].title, "iPhone");
        assert_eq!(devices[0].app.as_deref(), Some("Happ 3.2"));
        assert_eq!(devices[0].added_at, now - Duration::days(30));
        assert_eq!(devices[1].title, "iPhone 15 Pro");
        let err = ops
            .disconnect_device(subscription.id, "hw-unknown", owner)
            .await
            .unwrap_err();
        assert_eq!(code(&err), "not_found");
        ops.disconnect_device(subscription.id, "hw-1", owner)
            .await
            .unwrap();
        let left: Vec<String> = env
            .fake
            .devices(RemnaUserId(1))
            .into_iter()
            .map(|d| d.hwid)
            .collect();
        assert_eq!(left, vec!["hw-2".to_string()]);
        assert_eq!(
            env.subscription(subscription.id).await.mirror.devices_count,
            Some(1)
        );
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].action, actions::DISCONNECT_DEVICE);
        assert_eq!(audit[0].details["title"], "iPhone");
        env.finish().await;
    }

    async fn tariff_with_extras(env: &Env) -> sn_domain::tariff::Tariff {
        let mut tx = env.db.store.begin().await.unwrap();
        let category = tx
            .tariffs()
            .create_category(&NewCategory {
                name: "Основные".into(),
                sort: 0,
                visible: true,
            })
            .await
            .unwrap();
        let tariff = tx
            .tariffs()
            .create(&NewTariff {
                traffic_addon: None,
                category_id: category.id,
                name: "Стандарт".into(),
                description: None,
                on_sale: true,
                sort: 0,
                devices: DevicePolicy {
                    included: 3,
                    extra: Some(ExtraDevices {
                        max: 2,
                        monthly: CurrencyPrices {
                            usd: Some(99),
                            rub: Some(4_900),
                        },
                        monthly_stars: None,
                    }),
                },
                traffic: TrafficPolicy {
                    limit_bytes: Some(100 * GIB),
                    reset: sn_domain::traffic::TrafficReset::Month,
                },
                squads: vec![sn_domain::ids::SquadId(Uuid::from_u128(1))],
                external_squad: None,
                periods: vec![NewPeriod {
                    days: 90,
                    prices: CurrencyPrices {
                        usd: Some(699),
                        rub: Some(49_900),
                    },
                    stars: None,
                    sort: 0,
                    active: true,
                }],
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        tariff
    }

    #[tokio::test]
    async fn уменьшение_лимита_удаляет_лишние_устройства_старые_первыми() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let tariff = tariff_with_extras(&env).await;
        let now = Utc::now();
        let mut user = sample_user(1, "sn_limits");
        user.hwid_device_limit = Some(5);
        user.status = UserStatus::Limited;
        user.traffic_limit_bytes = (100 * GIB) as i64;
        user.user_traffic.used_traffic_bytes = (100 * GIB) as i64;
        for (index, age) in [3_i64, 10, 1, 7, 5].into_iter().enumerate() {
            let mut device = sample_device(
                RemnaUserId(1),
                &format!("hw-{index}"),
                now - Duration::days(30),
            );
            device.updated_at = now - Duration::days(age);
            env.fake.add_device(device);
        }
        let subscription = env.linked(client.id, user).await;
        {
            let mut conn = env.db.store.conn().await.unwrap();
            conn.subscriptions()
                .apply_purchase(
                    subscription.id,
                    &PurchaseApplied {
                        tariff_id: Some(tariff.id),
                        kind: SubscriptionKind::Paid,
                        extra_devices: 2,
                        period_started_at: now,
                        period_days: 90,
                    },
                )
                .await
                .unwrap();
        }
        env.fake.fail_on("update_user", Fault::TimeoutAfterApply);
        let view = env
            .services
            .subscriptions()
            .change_limits(
                subscription.id,
                &LimitsChange {
                    traffic_limit_bytes: Some(Some(200 * GIB)),
                    extra_devices: Some(0),
                    devices: None,
                },
                ADMIN,
            )
            .await
            .unwrap();
        let fresh = env.fake.user(RemnaUserId(1)).unwrap();
        assert_eq!(fresh.hwid_device_limit, Some(3));
        assert_eq!(fresh.traffic_limit_bytes, (200 * GIB) as i64);
        assert_eq!(fresh.status, UserStatus::Active);
        let left: Vec<String> = env
            .fake
            .devices(RemnaUserId(1))
            .into_iter()
            .map(|d| d.hwid)
            .collect();
        assert_eq!(left, vec!["hw-0", "hw-2", "hw-4"]);
        assert_eq!(view.subscription.extra_devices, 0);
        assert_eq!(view.subscription.mirror.devices_count, Some(3));
        assert_eq!(view.subscription.mirror.hwid_limit, Some(3));
        assert_eq!(env.fake.calls_of("update_user").len(), 1);
        assert_eq!(
            env.status_events(client.id).await,
            vec![(
                Some(SubscriptionStatus::Limited),
                SubscriptionStatus::Active
            )]
        );
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].action, actions::CHANGE_LIMITS);
        assert_eq!(audit[0].details["removed_devices"], json!(["hw-1", "hw-3"]));
        env.finish().await;
    }

    #[tokio::test]
    async fn лимиты_без_оплаченного_периода_записывают_доп_устройства() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_noperiod")).await;
        let ops = env.services.subscriptions();
        let view = ops
            .change_limits(
                subscription.id,
                &LimitsChange {
                    traffic_limit_bytes: None,
                    extra_devices: Some(2),
                    devices: None,
                },
                ADMIN,
            )
            .await
            .unwrap();
        assert_eq!(view.subscription.extra_devices, 2);
        assert_eq!(view.subscription.period_started_at, None);
        let user = env.fake.user(RemnaUserId(1)).unwrap();
        assert_eq!(user.hwid_device_limit, Some(2));
        let view = ops
            .change_limits(
                subscription.id,
                &LimitsChange {
                    traffic_limit_bytes: None,
                    extra_devices: Some(0),
                    devices: None,
                },
                ADMIN,
            )
            .await
            .unwrap();
        assert_eq!(view.subscription.extra_devices, 0);
        let view = ops
            .change_limits(
                subscription.id,
                &LimitsChange {
                    traffic_limit_bytes: Some(Some(GIB)),
                    extra_devices: None,
                    devices: None,
                },
                ADMIN,
            )
            .await
            .unwrap();
        assert_eq!(view.subscription.mirror.traffic_limit_bytes, Some(GIB));
        let err = ops
            .change_limits(
                subscription.id,
                &LimitsChange {
                    traffic_limit_bytes: Some(Some(0)),
                    extra_devices: None,
                    devices: None,
                },
                ADMIN,
            )
            .await
            .unwrap_err();
        assert_eq!(code(&err), "subscription.traffic_zero");
        env.finish().await;
    }

    #[tokio::test]
    async fn блокировка_и_разблокировка_всех_подписок_клиента() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let active = env.linked(client.id, sample_user(1, "sn_active")).await;
        let soon_expired = env.linked(client.id, sample_user(2, "sn_soon")).await;
        let unlinked = {
            let mut conn = env.db.store.conn().await.unwrap();
            conn.subscriptions()
                .create(&NewSubscription {
                    client_id: client.id,
                    tariff_id: None,
                    kind: SubscriptionKind::Paid,
                    extra_devices: 0,
                    autorenew: AutoRenewState::default(),
                })
                .await
                .unwrap()
        };
        let ops = env.services.subscriptions();
        for id in [active.id, soon_expired.id] {
            ops.set_autorenew(id, true, None, ADMIN).await.unwrap();
        }
        env.fake.fail_on("disable_user", Fault::TimeoutAfterApply);
        ops.block_access(client.id, Some("спам")).await.unwrap();
        for id in [1, 2] {
            assert_eq!(
                env.fake.user(RemnaUserId(id)).unwrap().status,
                UserStatus::Disabled
            );
        }
        assert_eq!(
            env.fake.calls_of("drop_connections"),
            vec![
                json!({"dropBy": {"by": "userIds", "userIds": [1, 2]}, "targetNodes": {"target": "allNodes"}})
            ]
        );
        for id in [active.id, soon_expired.id] {
            let stored = env.subscription(id).await;
            assert!(!stored.autorenew.enabled);
            assert_eq!(stored.mirror.status, Some(SubscriptionStatus::Disabled));
        }
        assert_eq!(env.subscription(unlinked.id).await.mirror.status, None);
        assert_eq!(
            env.status_events(client.id).await,
            vec![
                (
                    Some(SubscriptionStatus::Active),
                    SubscriptionStatus::Disabled
                ),
                (
                    Some(SubscriptionStatus::Active),
                    SubscriptionStatus::Disabled
                ),
            ]
        );
        let audit = env.audit("client", client.id.0).await;
        assert_eq!(audit[0].action, actions::BLOCK_ACCESS);
        assert_eq!(audit[0].details["reason"], "спам");

        let mut expire = UpdateUserRequest::new(RemnaUserId(2));
        expire.expire_at = Some(Utc::now() - Duration::hours(1));
        env.fake.update_user(&expire).await.unwrap();
        ops.restore_access(client.id).await.unwrap();
        assert_eq!(
            env.fake.user(RemnaUserId(1)).unwrap().status,
            UserStatus::Active
        );
        assert_eq!(
            env.fake.user(RemnaUserId(2)).unwrap().status,
            UserStatus::Expired
        );
        assert_eq!(env.fake.calls_of("enable_user").len(), 2);
        assert!(env.subscription(active.id).await.autorenew.enabled);
        assert!(!env.subscription(soon_expired.id).await.autorenew.enabled);
        assert_eq!(
            env.subscription(soon_expired.id).await.mirror.status,
            Some(SubscriptionStatus::Expired)
        );
        let audit = env.audit("client", client.id.0).await;
        assert_eq!(audit[0].action, actions::RESTORE_ACCESS);
        assert_eq!(audit[0].details["expired"], json!([soon_expired.id]));
        assert_eq!(
            audit[0].details["enabled"],
            json!([active.id, soon_expired.id])
        );
        let events = env.status_events(client.id).await;
        assert_eq!(
            events[events.len() - 2..],
            [
                (
                    Some(SubscriptionStatus::Disabled),
                    SubscriptionStatus::Active
                ),
                (
                    Some(SubscriptionStatus::Disabled),
                    SubscriptionStatus::Expired
                ),
            ]
        );
        env.finish().await;
    }

    #[tokio::test]
    async fn блокировка_уже_отключённой_подписки_проходит_без_ошибки() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let mut user = sample_user(1, "sn_already_disabled");
        user.status = UserStatus::Disabled;
        let disabled = env.linked(client.id, user).await;
        let active = env
            .linked(client.id, sample_user(2, "sn_still_active"))
            .await;
        let ops = env.services.subscriptions();
        ops.block_access(client.id, Some("возврат")).await.unwrap();
        for id in [1, 2] {
            assert_eq!(
                env.fake.user(RemnaUserId(id)).unwrap().status,
                UserStatus::Disabled
            );
        }
        for id in [disabled.id, active.id] {
            assert_eq!(
                env.subscription(id).await.mirror.status,
                Some(SubscriptionStatus::Disabled)
            );
        }
        let audit = env.audit("client", client.id.0).await;
        assert_eq!(
            audit[0].details["disabled"],
            json!([disabled.id, active.id])
        );
        assert_eq!(audit[0].details["failed"], json!([]));
        assert_eq!(
            env.fake.calls_of("drop_connections"),
            vec![
                json!({"dropBy": {"by": "userIds", "userIds": [1, 2]}, "targetNodes": {"target": "allNodes"}})
            ]
        );
        ops.block_access(client.id, Some("повторно")).await.unwrap();
        assert_eq!(env.fake.calls_of("disable_user").len(), 4);
        env.finish().await;
    }

    #[tokio::test]
    async fn повторное_включение_и_отключение_в_remnawave_считаются_успехом() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        env.linked(client.id, sample_user(1, "sn_on")).await;
        let user = enable_remna_user(&*env.fake, RemnaUserId(1)).await.unwrap();
        assert_eq!(user.status, UserStatus::Active);
        assert_eq!(env.fake.calls_of("enable_user").len(), 1);
        let user = disable_remna_user(&*env.fake, RemnaUserId(1))
            .await
            .unwrap();
        assert_eq!(user.status, UserStatus::Disabled);
        let user = disable_remna_user(&*env.fake, RemnaUserId(1))
            .await
            .unwrap();
        assert_eq!(user.status, UserStatus::Disabled);
        assert_eq!(env.fake.calls_of("disable_user").len(), 2);
        assert!(
            disable_remna_user(&*env.fake, RemnaUserId(404))
                .await
                .unwrap_err()
                .is_not_found()
        );
        env.fake.fail_on("enable_user", Fault::ServerError);
        assert!(
            enable_remna_user(&*env.fake, RemnaUserId(1))
                .await
                .unwrap_err()
                .outcome_unknown()
        );
        env.finish().await;
    }

    #[tokio::test]
    async fn разблокировка_включает_и_истёкшие_за_время_блокировки() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let subscription = env.linked(client.id, sample_user(1, "sn_lapsed")).await;
        let ops = env.services.subscriptions();
        ops.block_access(client.id, None).await.unwrap();
        let mut expire = UpdateUserRequest::new(RemnaUserId(1));
        expire.expire_at = Some(Utc::now() - Duration::days(2));
        env.fake.update_user(&expire).await.unwrap();
        ops.restore_access(client.id).await.unwrap();
        assert_eq!(
            env.fake.user(RemnaUserId(1)).unwrap().status,
            UserStatus::Expired
        );
        assert_eq!(
            env.subscription(subscription.id).await.mirror.status,
            Some(SubscriptionStatus::Expired)
        );
        let audit = env.audit("client", client.id.0).await;
        assert_eq!(audit[0].details["enabled"], json!([subscription.id]));
        assert_eq!(audit[0].details["expired"], json!([subscription.id]));
        env.finish().await;
    }

    fn squad_with_nodes(env: &Env, squad: Uuid, nodes: &[(u128, &str, &str)]) {
        let inbound = sn_remnawave::Inbound {
            uuid: Uuid::from_u128(squad.as_u128() ^ 0xA),
            profile_uuid: None,
            tag: format!("IN-{squad}"),
            kind: "vless".into(),
            network: Some("tcp".into()),
            security: Some("reality".into()),
            port: Some(443),
        };
        let mut squads: Vec<sn_remnawave::InternalSquad> = env
            .fake
            .squads()
            .into_iter()
            .filter(|s| s.uuid != squad)
            .collect();
        squads.push(sn_remnawave::InternalSquad {
            uuid: squad,
            name: format!("squad-{squad}"),
            view_position: 0,
            info: sn_remnawave::SquadInfo::default(),
            inbounds: vec![inbound.clone()],
        });
        env.fake.set_squads(squads);
        let mut details = env.fake.node_details();
        for (uuid, name, country) in nodes {
            let mut detail =
                sn_remnawave::NodeDetails::from(sample_node(Uuid::from_u128(*uuid), name, country));
            detail.config_profile = Some(sn_remnawave::NodeConfigProfile {
                active_config_profile_uuid: None,
                active_inbounds: vec![inbound.clone()],
            });
            details.push(detail);
        }
        env.fake.set_node_details(details);
    }

    fn in_squad(mut user: RemnaUser, squad: Uuid) -> RemnaUser {
        user.active_internal_squads = vec![sn_remnawave::SquadRef {
            uuid: squad,
            name: format!("squad-{squad}"),
        }];
        user
    }

    #[tokio::test]
    async fn карточка_подписки_показывает_страны_серверов() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let squad = Uuid::from_u128(0x51);
        squad_with_nodes(
            &env,
            squad,
            &[
                (1, "NL-1", "NL"),
                (2, "DE-1", "de"),
                (3, "NL-2", "NL"),
                (4, "XX-1", "XX"),
            ],
        );
        let subscription = env
            .linked(client.id, in_squad(sample_user(1, "sn_view"), squad))
            .await;
        let ops = env.services.subscriptions();
        let view = ops
            .view(subscription.id, Actor::Client(client.id), true)
            .await
            .unwrap();
        assert_eq!(view.countries, vec!["NL".to_string(), "DE".to_string()]);
        assert_eq!(
            view.subscription_url.as_deref(),
            Some("https://sub.example.com/short1")
        );
        let cached = ops.view(subscription.id, ADMIN, false).await.unwrap();
        assert!(cached.countries.is_empty());
        assert_eq!(cached.subscription_url, None);
        env.fake.delete_user(RemnaUserId(1)).await.unwrap();
        let gone = ops.view(subscription.id, ADMIN, true).await.unwrap();
        assert_eq!(gone.subscription_url, None);
        assert_eq!(
            code(&ops.add_days(subscription.id, 7, ADMIN).await.unwrap_err()),
            "not_found"
        );
        env.finish().await;
    }

    #[tokio::test]
    async fn повторный_просмотр_подписки_не_пишет_в_базу_и_не_спрашивает_страны() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let squad = Uuid::from_u128(0x52);
        squad_with_nodes(&env, squad, &[(11, "FI-1", "FI")]);
        let subscription = env
            .linked(client.id, in_squad(sample_user(5, "sn_again"), squad))
            .await;
        let ops = env.services.subscriptions();
        let me = Actor::Client(client.id);
        let first = ops.view(subscription.id, me, true).await.unwrap();
        assert_eq!(first.countries, vec!["FI".to_string()]);
        let synced = env.subscription(subscription.id).await.mirror.synced_at;
        assert!(synced.is_some());
        let before = env.fake.calls();
        let again = ops.view(subscription.id, me, true).await.unwrap();
        assert_eq!(again.countries, vec!["FI".to_string()]);
        assert_eq!(again.subscription, first.subscription);
        assert_eq!(
            again.subscription_url.as_deref(),
            Some("https://sub.example.com/short5")
        );
        let calls: Vec<String> = env.fake.calls().split_off(before.len());
        assert_eq!(calls, vec!["get_user".to_string()]);
        assert_eq!(
            env.subscription(subscription.id).await.mirror.synced_at,
            synced
        );
        let mut used = UpdateUserRequest::new(RemnaUserId(5));
        used.traffic_limit_bytes = Some(50 * GIB as i64);
        env.fake.update_user(&used).await.unwrap();
        let changed = ops.view(subscription.id, me, true).await.unwrap();
        assert_eq!(
            changed.subscription.mirror.traffic_limit_bytes,
            Some(50 * GIB)
        );
        assert_ne!(
            env.subscription(subscription.id).await.mirror.synced_at,
            synced
        );
        assert!(env.fake.calls_of("user_accessible_nodes").is_empty());
        env.finish().await;
    }

    #[tokio::test]
    async fn промокод_на_дни_продлевает_подписку_через_ядро() {
        let Some(env) = env().await else { return };
        let client = env.client().await;
        let user = sample_user(9, "sn_promo");
        let before = user.expire_at;
        let subscription = env.linked(client.id, user).await;
        let ctx = env.services.module_context();
        sn_modules::catalog::promo::create(
            &ctx,
            &sn_modules::catalog::promo::NewPromo {
                code: "WEEK7".into(),
                value: sn_modules::catalog::promo::NewPromoValue::Days { days: 7 },
                max_uses: None,
                per_client: 1,
                valid_from: None,
                valid_until: None,
                first_purchase_only: false,
                tariff_ids: Vec::new(),
                note: None,
            },
            None,
        )
        .await
        .unwrap();
        let activation =
            sn_modules::catalog::promo::activate(&ctx, client.id, " week7 ", subscription.id)
                .await
                .unwrap();
        assert_eq!(activation.days, 7);
        assert_eq!(
            env.fake.user(RemnaUserId(9)).unwrap().expire_at,
            before + Duration::days(7)
        );
        let audit = env.audit("subscription", subscription.id.0).await;
        assert_eq!(audit[0].action, actions::ADD_DAYS);
        assert_eq!(audit[0].actor_kind, "system");
        assert!(
            sn_modules::catalog::promo::activate(&ctx, client.id, "WEEK7", subscription.id)
                .await
                .is_err()
        );
        assert_eq!(
            env.fake.user(RemnaUserId(9)).unwrap().expire_at,
            before + Duration::days(7)
        );
        env.finish().await;
    }
}

#[cfg(test)]
mod limits_change_tests {
    use super::LimitsChange;

    #[test]
    fn пустой_лимит_трафика_снимает_лимит_а_пропуск_не_меняет() {
        let removed: LimitsChange =
            serde_json::from_value(serde_json::json!({ "traffic_limit_bytes": null })).unwrap();
        assert_eq!(removed.traffic_limit_bytes, Some(None));
        let kept: LimitsChange =
            serde_json::from_value(serde_json::json!({ "extra_devices": 2 })).unwrap();
        assert_eq!(kept.traffic_limit_bytes, None);
        assert_eq!(kept.extra_devices, Some(2));
        let set: LimitsChange =
            serde_json::from_value(serde_json::json!({ "traffic_limit_bytes": 1024 })).unwrap();
        assert_eq!(set.traffic_limit_bytes, Some(Some(1024)));
    }
}
