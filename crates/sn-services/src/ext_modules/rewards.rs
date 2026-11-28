use super::store::{self, ModuleRow};
use super::{ExtModulesOps, rejected};
use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::subscriptions::{LimitsChange, MAX_DEVICES};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sn_domain::access::{AdminPrincipal, Permission};
use sn_domain::ids::{ClientId, SubscriptionId};
use sn_modules::catalog::prize;
use sn_modules::catalog::promo::{self, PromoKind};
use sn_store::Db;

pub const REWARD_AUDIT: &str = "ext_module.reward";
pub const REFERENCE_MAX: usize = 80;
pub const LABEL_MAX: usize = 100;
pub const DAYS_MAX: u32 = 365;
pub const DEVICES_MAX: u32 = 10;
pub const PROMO_DAYS_MAX: u32 = 365;
const STALE_MINUTES: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reward {
    Days { days: u32 },
    Devices { devices: u32 },
    Promo { percent: u8, valid_days: u32 },
}

impl Reward {
    pub const fn kind(&self) -> &'static str {
        match self {
            Reward::Days { .. } => "days",
            Reward::Devices { .. } => "devices",
            Reward::Promo { .. } => "promo",
        }
    }

    fn check(&self) -> ServiceResult<()> {
        let fail = |message: String| Err(rejected("reward.invalid", message));
        match *self {
            Reward::Days { days } if !(1..=DAYS_MAX).contains(&days) => {
                fail(format!("дней — от 1 до {DAYS_MAX}"))
            }
            Reward::Devices { devices } if !(1..=DEVICES_MAX).contains(&devices) => {
                fail(format!("устройств — от 1 до {DEVICES_MAX}"))
            }
            Reward::Promo { percent, .. } if !(1..=100).contains(&percent) => {
                fail("скидка — от 1 до 100 %".into())
            }
            Reward::Promo { valid_days, .. } if !(1..=PROMO_DAYS_MAX).contains(&valid_days) => {
                fail(format!("промокод действует от 1 до {PROMO_DAYS_MAX} дней"))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RewardRequest {
    pub reference: String,
    pub client_id: ClientId,
    #[serde(default)]
    pub subscription_id: Option<SubscriptionId>,
    pub reward: Reward,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardStatus {
    Applied,
    Duplicate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Granted {
    pub subscription_id: Option<SubscriptionId>,
    pub expire_at: Option<DateTime<Utc>>,
    pub extra_devices: Option<u32>,
    pub device_limit: Option<u32>,
    pub code: Option<String>,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RewardResult {
    pub reference: String,
    pub status: RewardStatus,
    pub client_id: ClientId,
    pub reward: Reward,
    #[serde(flatten)]
    pub granted: Granted,
    pub applied_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RewardState {
    pub reference: String,
    pub state: String,
    pub client_id: ClientId,
    pub reward: Reward,
    pub granted: Option<Granted>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RewardRow {
    reference: String,
    client_id: i64,
    request: Value,
    status: String,
    result: Option<Value>,
    error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl RewardRow {
    fn reward(&self) -> ServiceResult<Reward> {
        serde_json::from_value(self.request.get("reward").cloned().unwrap_or(Value::Null))
            .map_err(|e| ServiceError::Internal(format!("награда не разобрана: {e}")))
    }

    fn granted(&self) -> Option<Granted> {
        self.result
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    fn same(&self, request: &RewardRequest) -> bool {
        let subscription = self
            .request
            .get("subscription_id")
            .and_then(Value::as_i64)
            .map(SubscriptionId);
        self.client_id == request.client_id.0
            && subscription == request.subscription_id
            && self.reward().is_ok_and(|r| r == request.reward)
    }

    fn done(&self, status: RewardStatus) -> ServiceResult<RewardResult> {
        Ok(RewardResult {
            reference: self.reference.clone(),
            status,
            client_id: ClientId(self.client_id),
            reward: self.reward()?,
            granted: self.granted().unwrap_or_default(),
            applied_at: self.updated_at,
        })
    }
}

const ROW_COLUMNS: &str =
    "reference, client_id, request, status, result, error, created_at, updated_at";

fn reference_of(raw: &str) -> ServiceResult<String> {
    let reference = raw.trim();
    let ok = !reference.is_empty()
        && reference.len() <= REFERENCE_MAX
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
    if !ok {
        return Err(rejected(
            "reward.reference",
            format!(
                "номер награды — от 1 до {REFERENCE_MAX} знаков: латинские буквы, цифры и - _ . :"
            ),
        ));
    }
    Ok(reference.to_string())
}

fn label_of(raw: Option<&str>) -> ServiceResult<Option<String>> {
    let Some(label) = raw.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > LABEL_MAX || label.chars().any(char::is_control) {
        return Err(rejected(
            "reward.label",
            format!("подпись награды — одна строка до {LABEL_MAX} символов"),
        ));
    }
    Ok(Some(label.to_string()))
}

pub fn device_total(
    subscription_limit: Option<u32>,
    included: Option<u32>,
    extra: u32,
    hwid: Option<u32>,
) -> Option<u32> {
    subscription_limit
        .or_else(|| included.map(|included| included + extra))
        .or(hwid)
}

enum Claim {
    Fresh,
    Done(RewardResult),
    Recheck(RewardRow),
}

impl ExtModulesOps<'_> {
    async fn reward_module(&self, principal: &AdminPrincipal) -> ServiceResult<ModuleRow> {
        let key_id = principal.api_key.ok_or_else(|| {
            rejected(
                "ext_modules.not_module",
                "Этот адрес — для ключа внешнего модуля",
            )
        })?;
        let mut conn = self.services.store().conn().await?;
        store::by_key(&mut conn, key_id).await?.ok_or_else(|| {
            rejected(
                "ext_modules.not_module",
                "Этот ключ не принадлежит внешнему модулю",
            )
        })
    }

    pub async fn reward_state(
        &self,
        principal: &AdminPrincipal,
        reference: &str,
    ) -> ServiceResult<RewardState> {
        let row = self.reward_module(principal).await?;
        if !principal.can(Permission::ClientsRead) {
            return Err(ServiceError::Forbidden);
        }
        let reference = reference_of(reference)?;
        let mut conn = self.services.store().conn().await?;
        let found: Option<RewardRow> = sqlx::query_as(&format!(
            "SELECT {ROW_COLUMNS} FROM ext_module_rewards WHERE module_id = $1 AND reference = $2"
        ))
        .bind(row.id())
        .bind(&reference)
        .fetch_optional(conn.conn())
        .await?;
        let found = found.ok_or(ServiceError::NotFound("награда"))?;
        Ok(RewardState {
            reward: found.reward()?,
            granted: found.granted(),
            reference: found.reference,
            state: found.status,
            client_id: ClientId(found.client_id),
            error: found.error,
            created_at: found.created_at,
            updated_at: found.updated_at,
        })
    }

    pub async fn reward(
        &self,
        principal: &AdminPrincipal,
        request: &RewardRequest,
    ) -> ServiceResult<RewardResult> {
        let row = self.reward_module(principal).await?;
        if !principal.can(Permission::ClientsWrite) {
            return Err(ServiceError::Forbidden);
        }
        if matches!(request.reward, Reward::Promo { .. }) && !principal.can(Permission::Marketing) {
            return Err(ServiceError::Forbidden);
        }
        let reference = reference_of(&request.reference)?;
        let label = label_of(request.label.as_deref())?;
        request.reward.check()?;
        {
            let mut conn = self.services.store().conn().await?;
            conn.clients().get(request.client_id).await?;
        }
        let module = row.id().to_string();
        let stored = json!({
            "client_id": request.client_id,
            "subscription_id": request.subscription_id,
            "reward": request.reward,
            "label": label,
        });
        match self.claim(&module, &reference, request, &stored).await? {
            Claim::Fresh => {}
            Claim::Done(result) => return Ok(result),
            Claim::Recheck(previous) => {
                if let Some(result) = self.recover(&module, &previous).await? {
                    return Ok(result);
                }
                return Err(rejected(
                    "reward.unknown",
                    "Прошлая выдача этой награды оборвалась, и неизвестно, дошла ли она до клиента. Проверьте подписку клиента и при необходимости выдайте вручную",
                ));
            }
        }
        let note = json!({
            "module": module,
            "module_name": row.manifest.name,
            "reference": reference,
            "label": label,
        });
        let applied = self
            .apply_reward(&row, request, &reference, label.as_deref(), &note)
            .await;
        let mut conn = self.services.store().conn().await?;
        match applied {
            Ok(granted) => {
                let now = self.services.now();
                sqlx::query(
                    "UPDATE ext_module_rewards SET status = 'applied', result = $3, error = NULL, updated_at = $4 \
                     WHERE module_id = $1 AND reference = $2",
                )
                .bind(&module)
                .bind(&reference)
                .bind(serde_json::to_value(&granted).unwrap_or(Value::Null))
                .bind(now)
                .execute(conn.conn())
                .await?;
                Ok(RewardResult {
                    reference,
                    status: RewardStatus::Applied,
                    client_id: request.client_id,
                    reward: request.reward,
                    granted,
                    applied_at: now,
                })
            }
            Err(err) => {
                sqlx::query(
                    "UPDATE ext_module_rewards SET status = 'failed', error = $3, updated_at = $4 \
                     WHERE module_id = $1 AND reference = $2 AND status = 'applying'",
                )
                .bind(&module)
                .bind(&reference)
                .bind(err.to_string().chars().take(300).collect::<String>())
                .bind(self.services.now())
                .execute(conn.conn())
                .await?;
                Err(err)
            }
        }
    }

    async fn claim(
        &self,
        module: &str,
        reference: &str,
        request: &RewardRequest,
        stored: &Value,
    ) -> ServiceResult<Claim> {
        let now = self.services.now();
        let mut tx = self.services.store().begin().await?;
        let inserted: Option<String> = sqlx::query_scalar(
            "INSERT INTO ext_module_rewards (module_id, reference, client_id, request, status, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 'applying', $5, $5) ON CONFLICT (module_id, reference) DO NOTHING RETURNING reference",
        )
        .bind(module)
        .bind(reference)
        .bind(request.client_id.0)
        .bind(stored)
        .bind(now)
        .fetch_optional(tx.conn())
        .await?;
        if inserted.is_some() {
            tx.commit().await?;
            return Ok(Claim::Fresh);
        }
        let previous: RewardRow = sqlx::query_as(&format!(
            "SELECT {ROW_COLUMNS} FROM ext_module_rewards WHERE module_id = $1 AND reference = $2 FOR UPDATE"
        ))
        .bind(module)
        .bind(reference)
        .fetch_one(tx.conn())
        .await?;
        if !previous.same(request) {
            return Err(rejected(
                "reward.mismatch",
                "Этот номер награды уже занят другой наградой или другим клиентом",
            ));
        }
        match previous.status.as_str() {
            "applied" => previous.done(RewardStatus::Duplicate).map(Claim::Done),
            "failed" => {
                sqlx::query(
                    "UPDATE ext_module_rewards SET status = 'applying', request = $3, updated_at = $4 \
                     WHERE module_id = $1 AND reference = $2",
                )
                .bind(module)
                .bind(reference)
                .bind(stored)
                .bind(now)
                .execute(tx.conn())
                .await?;
                tx.commit().await?;
                Ok(Claim::Fresh)
            }
            _ if previous.updated_at > now - Duration::minutes(STALE_MINUTES) => Err(rejected(
                "reward.in_progress",
                "Эта награда уже выдаётся: повторите запрос через минуту",
            )),
            _ => Ok(Claim::Recheck(previous)),
        }
    }

    async fn recover(
        &self,
        module: &str,
        previous: &RewardRow,
    ) -> ServiceResult<Option<RewardResult>> {
        let mut conn = self.services.store().conn().await?;
        let found: Option<(Option<String>, Option<i64>, Value)> = sqlx::query_as(
            "SELECT entity_kind, entity_id, details FROM audit_log \
             WHERE action IN ('subscription.add_days', 'subscription.change_limits', $3) \
             AND details ->> 'module' = $1 AND details ->> 'reference' = $2 ORDER BY id DESC LIMIT 1",
        )
        .bind(module)
        .bind(&previous.reference)
        .bind(REWARD_AUDIT)
        .fetch_optional(conn.conn())
        .await?;
        let Some((entity_kind, entity_id, details)) = found else {
            return Ok(None);
        };
        let granted = Granted {
            subscription_id: (entity_kind.as_deref() == Some("subscription"))
                .then_some(entity_id)
                .flatten()
                .map(SubscriptionId),
            expire_at: details
                .get("expire_at")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok()),
            extra_devices: details
                .get("extra_devices")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok()),
            device_limit: details
                .get("device_limit")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok()),
            code: details
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string),
            valid_until: details
                .get("valid_until")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok()),
        };
        let now = self.services.now();
        sqlx::query(
            "UPDATE ext_module_rewards SET status = 'applied', result = $3, error = NULL, updated_at = $4 \
             WHERE module_id = $1 AND reference = $2",
        )
        .bind(module)
        .bind(&previous.reference)
        .bind(serde_json::to_value(&granted).unwrap_or(Value::Null))
        .bind(now)
        .execute(conn.conn())
        .await?;
        Ok(Some(RewardResult {
            reference: previous.reference.clone(),
            status: RewardStatus::Duplicate,
            client_id: ClientId(previous.client_id),
            reward: previous.reward()?,
            granted,
            applied_at: now,
        }))
    }

    async fn reward_target(
        &self,
        client_id: ClientId,
        chosen: Option<SubscriptionId>,
    ) -> ServiceResult<SubscriptionId> {
        let mut conn = self.services.store().conn().await?;
        if let Some(id) = chosen {
            let subscription = match conn.subscriptions().get(id).await {
                Ok(subscription) => Some(subscription),
                Err(err) if err.is_not_found() => None,
                Err(err) => return Err(err.into()),
            };
            return match subscription {
                Some(s)
                    if s.client_id == client_id && s.ended_at.is_none() && s.remna.is_some() =>
                {
                    Ok(s.id)
                }
                _ => Err(rejected(
                    "reward.subscription",
                    "Эта подписка не принадлежит клиенту, завершена или не связана с Remnawave",
                )),
            };
        }
        prize::live_subscription(&mut conn, client_id, None)
            .await?
            .map(|live| live.id)
            .ok_or_else(|| {
                rejected(
                    "reward.no_subscription",
                    "У клиента нет действующей подписки, к которой можно добавить награду",
                )
            })
    }

    async fn apply_reward(
        &self,
        row: &ModuleRow,
        request: &RewardRequest,
        reference: &str,
        label: Option<&str>,
        note: &Value,
    ) -> ServiceResult<Granted> {
        let subscriptions = self.services.subscriptions();
        match request.reward {
            Reward::Days { days } => {
                let target = self
                    .reward_target(request.client_id, request.subscription_id)
                    .await?;
                let view = subscriptions
                    .add_days_noted(target, days, Actor::System, Some(note))
                    .await?;
                Ok(Granted {
                    subscription_id: Some(target),
                    expire_at: view.subscription.mirror.expire_at,
                    ..Granted::default()
                })
            }
            Reward::Devices { devices } => {
                let target = self
                    .reward_target(request.client_id, request.subscription_id)
                    .await?;
                let (subscription, included) = {
                    let mut conn = self.services.store().conn().await?;
                    let subscription = conn.subscriptions().get(target).await?;
                    let included = match subscription.tariff_id {
                        Some(tariff_id) => match conn.tariffs().get(tariff_id).await {
                            Ok(tariff) => Some(tariff.devices.included),
                            Err(err) if err.is_not_found() => None,
                            Err(err) => return Err(err.into()),
                        },
                        None => None,
                    };
                    (subscription, included)
                };
                let current = device_total(
                    subscription.device_limit,
                    included,
                    subscription.extra_devices,
                    subscription.mirror.hwid_limit,
                )
                .ok_or_else(|| {
                    rejected(
                        "reward.devices_unknown",
                        "У подписки без тарифа неизвестно число устройств: добавьте устройство вручную",
                    )
                })?;
                let total = current.saturating_add(devices);
                if total > MAX_DEVICES {
                    return Err(rejected(
                        "reward.devices_max",
                        format!("у подписки не бывает больше {MAX_DEVICES} устройств"),
                    ));
                }
                let change = LimitsChange {
                    traffic_limit_bytes: None,
                    extra_devices: None,
                    devices: Some(total),
                };
                let view = subscriptions
                    .change_limits_noted(target, &change, Actor::System, Some(note))
                    .await?;
                Ok(Granted {
                    subscription_id: Some(target),
                    extra_devices: Some(view.subscription.extra_devices),
                    device_limit: device_total(
                        view.subscription.device_limit,
                        included,
                        view.subscription.extra_devices,
                        view.subscription.mirror.hwid_limit,
                    ),
                    ..Granted::default()
                })
            }
            Reward::Promo {
                percent,
                valid_days,
            } => {
                let now = self.services.now();
                let valid_until = now + Duration::days(i64::from(valid_days));
                let mut tx = self.services.store().begin().await?;
                if !prize::promo_enabled(&mut tx).await? {
                    return Err(rejected(
                        "reward.promo_off",
                        "Модуль «Промокоды» выключен: личный промокод не выпустить",
                    ));
                }
                let text = match label {
                    Some(label) => format!("Модуль «{}»: {label}", row.manifest.name),
                    None => format!("Модуль «{}»", row.manifest.name),
                };
                let code = promo::issue_personal(
                    &mut tx,
                    PromoKind::Percent { percent },
                    Some(valid_until),
                    &text,
                    request.client_id,
                )
                .await
                .map_err(ServiceError::from)?
                .code;
                let mut details = note.clone();
                if let Value::Object(map) = &mut details {
                    map.insert("kind".into(), json!("promo"));
                    map.insert("percent".into(), json!(percent));
                    map.insert("valid_days".into(), json!(valid_days));
                    map.insert("code".into(), json!(code));
                    map.insert("valid_until".into(), json!(valid_until));
                }
                tx.audit()
                    .record(&Actor::System.audit(
                        REWARD_AUDIT,
                        "client",
                        Some(request.client_id.0),
                        details,
                    ))
                    .await?;
                let granted = Granted {
                    code: Some(code),
                    valid_until: Some(valid_until),
                    ..Granted::default()
                };
                sqlx::query(
                    "UPDATE ext_module_rewards SET status = 'applied', result = $3, error = NULL, updated_at = $4 \
                     WHERE module_id = $1 AND reference = $2",
                )
                .bind(row.id())
                .bind(reference)
                .bind(serde_json::to_value(&granted).unwrap_or(Value::Null))
                .bind(now)
                .execute(tx.conn())
                .await?;
                tx.commit().await?;
                Ok(granted)
            }
        }
    }
}
