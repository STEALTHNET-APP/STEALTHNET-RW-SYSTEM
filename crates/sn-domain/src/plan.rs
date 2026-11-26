use crate::DomainError;
use crate::ids::{CategoryId, RemnaUserId, SquadId, SubscriptionId};
use crate::order::OrderKind;
use crate::subscription::{SubscriptionKind, SubscriptionStatus};
use crate::tariff::TariffSnapshot;
use crate::traffic::{TrafficReset, TrafficStrategy, unused_extra};
use chrono::{DateTime, Duration, Months, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    New,
    Renewal,
    Change,
}

impl PlanKind {
    pub const fn order_kind(self) -> OrderKind {
        match self {
            PlanKind::New => OrderKind::New,
            PlanKind::Renewal => OrderKind::Renewal,
            PlanKind::Change => OrderKind::Change,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentTariff {
    pub kind: SubscriptionKind,
    pub category_id: Option<CategoryId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldSubscription {
    pub id: SubscriptionId,
    pub category_id: Option<CategoryId>,
}

pub fn single_target(held: &[HeldSubscription], target: CategoryId) -> Option<SubscriptionId> {
    held.iter()
        .filter(|h| h.category_id == Some(target))
        .map(|h| h.id)
        .max()
        .or_else(|| held.iter().map(|h| h.id).max())
}

pub fn classify(target_category: CategoryId, current: Option<CurrentTariff>) -> PlanKind {
    match current {
        None => PlanKind::New,
        Some(c) if c.kind == SubscriptionKind::Trial => PlanKind::Change,
        Some(c) if c.category_id == Some(target_category) => PlanKind::Renewal,
        Some(_) => PlanKind::Change,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaUserState {
    pub user_id: RemnaUserId,
    pub status: SubscriptionStatus,
    pub expire_at: DateTime<Utc>,
    pub squads: Vec<SquadId>,
    pub traffic_limit_bytes: u64,
    pub traffic_used_bytes: u64,
    pub traffic_strategy: TrafficStrategy,
    pub hwid_device_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewUserProfile {
    pub username: String,
    pub telegram_id: Option<i64>,
    pub email: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldDevices {
    pub limit: u32,
    pub until: DateTime<Utc>,
}

pub const RECURRING_GRACE_HOURS: u32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cycle {
    Days(u32),
    Months(u32),
}

impl Cycle {
    pub fn after(self, at: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Cycle::Days(days) => at + Duration::days(i64::from(days)),
            Cycle::Months(months) => at
                .checked_add_months(Months::new(months))
                .unwrap_or_else(|| at + Duration::days(i64::from(months) * 31)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grace {
    pub hours: u32,
    pub carried: bool,
    #[serde(default)]
    pub cycle: Option<Cycle>,
    #[serde(default)]
    pub charged_at: Option<DateTime<Utc>>,
}

impl Grace {
    pub const fn first() -> Self {
        Grace {
            hours: RECURRING_GRACE_HOURS,
            carried: false,
            cycle: None,
            charged_at: None,
        }
    }

    pub const fn renewal() -> Self {
        Grace {
            hours: RECURRING_GRACE_HOURS,
            carried: true,
            cycle: None,
            charged_at: None,
        }
    }

    pub const fn every(self, cycle: Option<Cycle>, charged_at: Option<DateTime<Utc>>) -> Self {
        Grace {
            cycle,
            charged_at,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlanInput<'a> {
    pub grace: Option<Grace>,
    pub kind: PlanKind,
    pub snapshot: &'a TariffSnapshot,
    pub extra_devices: u32,
    pub held_devices: Option<HeldDevices>,
    pub extra_traffic: u64,
    pub paid_at: DateTime<Utc>,
    pub current: Option<&'a RemnaUserState>,
    pub keep_squads: &'a [SquadId],
    pub profile: &'a NewUserProfile,
    pub tag: Option<&'a str>,
    pub manual_devices: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemnaTarget {
    pub expire_at: DateTime<Utc>,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
    pub traffic_limit_bytes: u64,
    pub traffic_strategy: TrafficStrategy,
    pub hwid_device_limit: Option<u32>,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanAction {
    Create {
        username: String,
        telegram_id: Option<i64>,
        email: Option<String>,
        description: String,
    },
    Update {
        user_id: RemnaUserId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FulfillmentPlan {
    pub kind: PlanKind,
    pub action: PlanAction,
    pub target: RemnaTarget,
    pub reset_traffic: bool,
    pub activate: bool,
    pub period_started_at: DateTime<Utc>,
    pub period_days: u32,
    #[serde(default)]
    pub base_expire_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub device_hold: Option<DeviceHold>,
    #[serde(default)]
    pub extra_traffic_bytes: u64,
    #[serde(default)]
    pub keep_traffic_window: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHold {
    pub held_limit: u32,
    pub until: DateTime<Utc>,
    pub limit_after: u32,
}

pub fn renewal_base(current_expire_at: DateTime<Utc>, paid_at: DateTime<Utc>) -> DateTime<Utc> {
    current_expire_at.max(paid_at)
}

pub fn whole_seconds(at: DateTime<Utc>) -> DateTime<Utc> {
    if at.timestamp_subsec_nanos() == 0 {
        return at;
    }
    DateTime::from_timestamp(at.timestamp() + 1, 0).unwrap_or(at)
}

pub fn new_expire_at(
    kind: PlanKind,
    current_expire_at: Option<DateTime<Utc>>,
    paid_at: DateTime<Utc>,
    days: u32,
) -> DateTime<Utc> {
    let base = match (kind, current_expire_at) {
        (PlanKind::Renewal, Some(current)) => renewal_base(current, paid_at),
        _ => paid_at,
    };
    whole_seconds(base + Duration::days(days as i64))
}

pub fn graced_expire_at(
    kind: PlanKind,
    current_expire_at: Option<DateTime<Utc>>,
    paid_at: DateTime<Utc>,
    days: u32,
    grace: Option<Grace>,
) -> DateTime<Utc> {
    let Some(grace) = grace else {
        return new_expire_at(kind, current_expire_at, paid_at, days);
    };
    let buffer = Duration::hours(i64::from(grace.hours));
    let at = grace
        .charged_at
        .map_or(paid_at, |charged| charged.min(paid_at));
    let term = match grace.cycle {
        Some(cycle) => cycle.after(at) - at,
        None => Duration::days(i64::from(days)),
    };
    let current = match kind {
        PlanKind::Renewal => current_expire_at,
        _ => None,
    };
    let end = match current {
        Some(current) if grace.carried => current.max(at + buffer) + term,
        Some(current) => current.max(at) + buffer + term,
        None => at + buffer + term,
    };
    whole_seconds(end)
}

fn merge_squads(primary: &[SquadId], extra: &[SquadId]) -> Vec<SquadId> {
    let mut out: Vec<SquadId> = Vec::with_capacity(primary.len() + extra.len());
    for squad in primary.iter().chain(extra) {
        if !out.contains(squad) {
            out.push(*squad);
        }
    }
    out
}

pub fn plan(input: &PlanInput<'_>) -> Result<FulfillmentPlan, DomainError> {
    let snapshot = input.snapshot;
    if input.extra_devices > snapshot.max_extra_devices {
        return Err(DomainError::TooManyExtraDevices {
            max: snapshot.max_extra_devices,
        });
    }
    if snapshot.squads.is_empty() {
        return Err(DomainError::invalid(
            "squads",
            "у тарифа нет сквадов: подписка выдалась бы без серверов",
        ));
    }
    let traffic = snapshot.traffic;
    let tariff_limit = traffic.remna_limit_bytes();
    let strategy = traffic.reset.strategy();
    let paid_devices = input.manual_devices.unwrap_or_else(|| {
        snapshot
            .devices_included
            .saturating_add(input.extra_devices)
    });
    let hwid_device_limit = (paid_devices > 0).then_some(paid_devices);
    let tag = input.tag.map(str::to_string);

    let Some(current) = input.current else {
        let expire_at =
            graced_expire_at(input.kind, None, input.paid_at, snapshot.days, input.grace);
        return Ok(FulfillmentPlan {
            kind: input.kind,
            action: PlanAction::Create {
                username: input.profile.username.clone(),
                telegram_id: input.profile.telegram_id,
                email: input.profile.email.clone(),
                description: input.profile.description.clone(),
            },
            target: RemnaTarget {
                expire_at,
                squads: snapshot.squads.clone(),
                external_squad: snapshot.external_squad,
                traffic_limit_bytes: tariff_limit,
                traffic_strategy: strategy,
                hwid_device_limit,
                tag,
            },
            reset_traffic: false,
            activate: false,
            period_started_at: input.paid_at,
            period_days: snapshot.days,
            base_expire_at: None,
            device_hold: None,
            extra_traffic_bytes: 0,
            keep_traffic_window: false,
        });
    };

    let renewal = input.kind == PlanKind::Renewal;
    let expire_at = graced_expire_at(
        input.kind,
        Some(current.expire_at),
        input.paid_at,
        snapshot.days,
        input.grace,
    );
    let squads = if renewal {
        merge_squads(&snapshot.squads, input.keep_squads)
    } else {
        snapshot.squads.clone()
    };
    let traffic_mode_changed =
        current.traffic_limit_bytes == 0 || current.traffic_strategy != strategy;
    let carry = snapshot.traffic_leftover.carries();
    let unused = unused_extra(
        current.traffic_limit_bytes,
        current.traffic_used_bytes,
        input.extra_traffic,
    );
    let kept = if carry { unused } else { 0 };
    let (traffic_limit_bytes, reset_traffic, extra_traffic_bytes, keep_traffic_window) = if !renewal
    {
        (tariff_limit, true, 0, false)
    } else if tariff_limit == 0 {
        (0, false, 0, false)
    } else if traffic_mode_changed {
        (tariff_limit, true, 0, false)
    } else {
        match traffic.reset {
            TrafficReset::OnPayment => (
                tariff_limit
                    .checked_add(kept)
                    .ok_or(DomainError::AmountOverflow)?,
                true,
                kept,
                false,
            ),
            TrafficReset::NoReset => (
                current
                    .traffic_limit_bytes
                    .saturating_sub(unused - kept)
                    .checked_add(tariff_limit)
                    .ok_or(DomainError::AmountOverflow)?,
                false,
                kept,
                false,
            ),
            _ => (
                tariff_limit
                    .checked_add(input.extra_traffic)
                    .ok_or(DomainError::AmountOverflow)?,
                false,
                input.extra_traffic,
                true,
            ),
        }
    };
    let device_hold = input
        .held_devices
        .filter(|held| {
            renewal && paid_devices > 0 && held.until > input.paid_at && held.limit > paid_devices
        })
        .map(|held| DeviceHold {
            held_limit: held.limit,
            until: held.until,
            limit_after: paid_devices,
        });
    let hwid_device_limit = match device_hold {
        Some(hold) => Some(hold.held_limit),
        None => hwid_device_limit,
    };
    let traffic_left = traffic_limit_bytes == 0
        || reset_traffic
        || traffic_limit_bytes > current.traffic_used_bytes;
    let activate = match current.status {
        SubscriptionStatus::Expired => true,
        SubscriptionStatus::Limited => traffic_left,
        SubscriptionStatus::Active | SubscriptionStatus::Disabled => false,
    };
    let period_days = ((expire_at - input.paid_at).num_seconds().max(0) + 86_399) / 86_400;

    Ok(FulfillmentPlan {
        kind: input.kind,
        action: PlanAction::Update {
            user_id: current.user_id,
        },
        target: RemnaTarget {
            expire_at,
            squads,
            external_squad: snapshot.external_squad,
            traffic_limit_bytes,
            traffic_strategy: strategy,
            hwid_device_limit,
            tag,
        },
        reset_traffic,
        activate,
        period_started_at: input.paid_at,
        period_days: period_days as u32,
        base_expire_at: Some(current.expire_at),
        device_hold,
        extra_traffic_bytes,
        keep_traffic_window,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PeriodId;
    use crate::tariff::fixtures::{standard, unblock};
    use crate::traffic::GIB;
    use chrono::TimeZone;
    use uuid::Uuid;

    fn profile() -> NewUserProfile {
        NewUserProfile {
            username: "sn_test".into(),
            telegram_id: Some(5_093_118_422),
            email: None,
            description: "stealthnet client".into(),
        }
    }

    fn paid_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 14, 52, 0).unwrap()
    }

    fn current(status: SubscriptionStatus, expire_at: DateTime<Utc>) -> RemnaUserState {
        RemnaUserState {
            user_id: RemnaUserId(77),
            status,
            expire_at,
            squads: vec![SquadId(Uuid::from_u128(1)), SquadId(Uuid::from_u128(9))],
            traffic_limit_bytes: 300 * GIB,
            traffic_used_bytes: 300 * GIB,
            traffic_strategy: TrafficStrategy::Month,
            hwid_device_limit: Some(3),
        }
    }

    #[test]
    fn категория_решает_продление_или_смену() {
        let same = CurrentTariff {
            kind: SubscriptionKind::Paid,
            category_id: Some(CategoryId(1)),
        };
        assert_eq!(classify(CategoryId(1), Some(same)), PlanKind::Renewal);
        assert_eq!(classify(CategoryId(2), Some(same)), PlanKind::Change);
        assert_eq!(classify(CategoryId(1), None), PlanKind::New);
        let trial = CurrentTariff {
            kind: SubscriptionKind::Trial,
            category_id: None,
        };
        assert_eq!(classify(CategoryId(1), Some(trial)), PlanKind::Change);
    }

    #[test]
    fn одна_подписка_берётся_по_категории_иначе_последняя() {
        let held = [
            HeldSubscription {
                id: SubscriptionId(3),
                category_id: Some(CategoryId(1)),
            },
            HeldSubscription {
                id: SubscriptionId(7),
                category_id: Some(CategoryId(2)),
            },
            HeldSubscription {
                id: SubscriptionId(5),
                category_id: None,
            },
        ];
        assert_eq!(single_target(&held, CategoryId(1)), Some(SubscriptionId(3)));
        assert_eq!(single_target(&held, CategoryId(2)), Some(SubscriptionId(7)));
        assert_eq!(single_target(&held, CategoryId(9)), Some(SubscriptionId(7)));
        assert_eq!(
            single_target(&held[2..], CategoryId(1)),
            Some(SubscriptionId(5))
        );
        assert_eq!(single_target(&[], CategoryId(1)), None);
    }

    #[test]
    fn продление_прибавляет_дни_к_дате_окончания() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        let p = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices: 2,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[SquadId(Uuid::from_u128(9))],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert_eq!(
            p.target.expire_at,
            Utc.with_ymd_and_hms(2027, 3, 16, 0, 0, 0).unwrap()
        );
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.target.squads.len(), 3);
        assert!(!p.reset_traffic);
        assert!(!p.activate);
    }

    #[test]
    fn истёкшая_подписка_продлевается_от_даты_оплаты() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expired = Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Expired, expired);
        let p = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert_eq!(p.target.expire_at, paid_at() + Duration::days(30));
        assert!(p.activate);
    }

    #[test]
    fn смена_тарифа_с_даты_оплаты_и_сбросом_трафика() {
        let tariff = unblock();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(32)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Limited, expire);
        let p = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Change,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[SquadId(Uuid::from_u128(9))],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert_eq!(p.target.expire_at, paid_at() + Duration::days(90));
        assert_eq!(p.target.squads, snapshot.squads);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.target.traffic_strategy, TrafficStrategy::Month);
        assert!(p.reset_traffic);
        assert!(p.activate);
    }

    #[test]
    fn без_пользователя_в_remnawave_создаём_нового() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(12)).unwrap());
        let p = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::New,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: None,
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert!(matches!(p.action, PlanAction::Create { .. }));
        assert_eq!(p.target.expire_at, paid_at() + Duration::days(90));
        assert_eq!(p.period_days, 90);
    }

    fn renewal(snapshot: &TariffSnapshot, state: &RemnaUserState) -> FulfillmentPlan {
        plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Renewal,
            snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid_at(),
            current: Some(state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap()
    }

    fn traffic_state(
        status: SubscriptionStatus,
        limit: u64,
        used: u64,
        strategy: TrafficStrategy,
    ) -> RemnaUserState {
        RemnaUserState {
            traffic_limit_bytes: limit,
            traffic_used_bytes: used,
            traffic_strategy: strategy,
            ..current(status, Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap())
        }
    }

    fn limited_snapshot(reset: TrafficReset) -> TariffSnapshot {
        let tariff = unblock();
        let mut snapshot = tariff.snapshot(tariff.period(PeriodId(31)).unwrap());
        snapshot.traffic.reset = reset;
        snapshot
    }

    #[test]
    fn продление_из_безлимита_в_тариф_с_лимитом_сбрасывает_счётчик() {
        let snapshot = limited_snapshot(TrafficReset::Month);
        let state = traffic_state(
            SubscriptionStatus::Active,
            0,
            1024 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.target.traffic_strategy, TrafficStrategy::Month);
        assert!(p.reset_traffic);
    }

    #[test]
    fn продление_со_сменой_сброса_трафика_сбрасывает_счётчик_и_включает() {
        let snapshot = limited_snapshot(TrafficReset::Month);
        let state = traffic_state(
            SubscriptionStatus::Limited,
            300 * GIB,
            300 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert!(p.reset_traffic);
        assert!(p.activate);

        let snapshot = limited_snapshot(TrafficReset::NoReset);
        let state = traffic_state(
            SubscriptionStatus::Active,
            300 * GIB,
            250 * GIB,
            TrafficStrategy::Month,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.target.traffic_strategy, TrafficStrategy::NoReset);
        assert!(p.reset_traffic);
    }

    #[test]
    fn продление_с_тем_же_сбросом_трафика_не_трогает_счётчик() {
        let snapshot = limited_snapshot(TrafficReset::Month);
        let state = traffic_state(
            SubscriptionStatus::Active,
            300 * GIB,
            120 * GIB,
            TrafficStrategy::Month,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert!(!p.reset_traffic);
        assert!(!p.activate);

        let snapshot = limited_snapshot(TrafficReset::NoReset);
        let state = traffic_state(
            SubscriptionStatus::Limited,
            300 * GIB,
            300 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 600 * GIB);
        assert!(!p.reset_traffic);
        assert!(p.activate);

        let snapshot = limited_snapshot(TrafficReset::OnPayment);
        let state = traffic_state(
            SubscriptionStatus::Active,
            300 * GIB,
            10 * GIB,
            TrafficStrategy::NoReset,
        );
        let p = renewal(&snapshot, &state);
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert!(p.reset_traffic);
    }

    fn renewal_holding(extra_devices: u32, held: Option<HeldDevices>) -> FulfillmentPlan {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices,
            held_devices: held,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap()
    }

    #[test]
    fn оплаченные_доп_устройства_держатся_до_конца_прежнего_срока() {
        let until = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let held = HeldDevices { limit: 5, until };
        let p = renewal_holding(0, Some(held));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(
            p.device_hold,
            Some(DeviceHold {
                held_limit: 5,
                until,
                limit_after: 3,
            })
        );
        assert_eq!(p.base_expire_at, Some(until));

        let p = renewal_holding(1, Some(held));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.device_hold.map(|h| h.limit_after), Some(4));

        let p = renewal_holding(2, Some(held));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.device_hold, None);

        let p = renewal_holding(2, Some(HeldDevices { limit: 4, until }));
        assert_eq!(p.target.hwid_device_limit, Some(5));
        assert_eq!(p.device_hold, None);

        let ended = HeldDevices {
            limit: 5,
            until: paid_at(),
        };
        let p = renewal_holding(0, Some(ended));
        assert_eq!(p.target.hwid_device_limit, Some(3));
        assert_eq!(p.device_hold, None);

        let p = renewal_holding(0, None);
        assert_eq!(p.target.hwid_device_limit, Some(3));
        assert_eq!(p.device_hold, None);
    }

    #[test]
    fn смена_тарифа_и_новая_подписка_доп_устройства_не_держат() {
        let tariff = unblock();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(31)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        let held = Some(HeldDevices {
            limit: 5,
            until: expire,
        });
        let change = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Change,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: held,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert_eq!(change.target.hwid_device_limit, Some(3));
        assert_eq!(change.device_hold, None);
        assert_eq!(change.base_expire_at, Some(expire));
        let fresh = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::New,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: held,
            paid_at: paid_at(),
            current: None,
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert_eq!(fresh.device_hold, None);
        assert_eq!(fresh.base_expire_at, None);
    }

    #[test]
    fn сохранённый_раньше_план_читается_без_новых_полей() {
        let p = renewal_holding(0, None);
        let mut json = serde_json::to_value(&p).unwrap();
        let object = json.as_object_mut().unwrap();
        object.remove("base_expire_at");
        object.remove("device_hold");
        let old: FulfillmentPlan = serde_json::from_value(json).unwrap();
        assert_eq!(old.base_expire_at, None);
        assert_eq!(old.device_hold, None);
        assert_eq!(old.target, p.target);
    }

    fn paid_with_microseconds() -> DateTime<Utc> {
        paid_at() + Duration::microseconds(600_007)
    }

    #[test]
    fn цель_плана_округляется_вверх_до_целой_секунды() {
        let exact = paid_at();
        assert_eq!(whole_seconds(exact), exact);
        assert_eq!(
            whole_seconds(exact + Duration::microseconds(1)),
            exact + Duration::seconds(1)
        );
        assert_eq!(
            whole_seconds(exact + Duration::nanoseconds(999_999_999)),
            exact + Duration::seconds(1)
        );
        let before_epoch = DateTime::from_timestamp(-10, 250_000_000).unwrap();
        assert_eq!(
            whole_seconds(before_epoch),
            DateTime::from_timestamp(-9, 0).unwrap()
        );
        let paid = paid_with_microseconds();
        let target = paid_at() + Duration::seconds(1) + Duration::days(30);
        assert_eq!(new_expire_at(PlanKind::Change, None, paid, 30), target);
        let expired = Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap();
        assert_eq!(
            new_expire_at(PlanKind::Renewal, Some(expired), paid, 30),
            target
        );
        let active =
            Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap() + Duration::milliseconds(250);
        assert_eq!(
            new_expire_at(PlanKind::Renewal, Some(active), paid, 30),
            Utc.with_ymd_and_hms(2027, 3, 16, 0, 0, 1).unwrap()
        );
    }

    #[test]
    fn план_от_оплаты_с_микросекундами_даёт_срок_без_долей_секунды() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let paid = paid_with_microseconds();
        let target = paid_at() + Duration::seconds(1) + Duration::days(30);
        let fresh = plan(&PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::New,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            paid_at: paid,
            current: None,
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap();
        assert_eq!(fresh.target.expire_at, target);
        assert_eq!(fresh.period_started_at, paid);
        assert_eq!(fresh.period_days, 30);
        let expired = current(
            SubscriptionStatus::Expired,
            Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap(),
        );
        for kind in [PlanKind::Renewal, PlanKind::Change] {
            let p = plan(&PlanInput {
                grace: None,
                extra_traffic: 0,
                kind,
                snapshot: &snapshot,
                extra_devices: 0,
                held_devices: None,
                paid_at: paid,
                current: Some(&expired),
                keep_squads: &[],
                profile: &profile(),
                tag: None,
                manual_devices: None,
            })
            .unwrap();
            assert_eq!(p.target.expire_at, target);
            assert_eq!(p.target.expire_at.timestamp_subsec_nanos(), 0);
            assert_eq!(p.period_days, 30);
            assert!(p.target.expire_at - paid >= Duration::days(30));
        }
    }

    #[test]
    fn тариф_без_сквадов_не_выдаётся() {
        let mut tariff = standard();
        tariff.squads.clear();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(12)).unwrap());
        assert!(
            plan(&PlanInput {
                grace: None,
                extra_traffic: 0,
                kind: PlanKind::New,
                snapshot: &snapshot,
                extra_devices: 0,
                held_devices: None,
                paid_at: paid_at(),
                current: None,
                keep_squads: &[],
                profile: &profile(),
                tag: None,
                manual_devices: None,
            })
            .is_err()
        );
    }

    fn with_bought_traffic(
        kind: PlanKind,
        reset: TrafficReset,
        leftover: crate::tariff::TrafficLeftover,
        (limit, used, extra): (u64, u64, u64),
    ) -> FulfillmentPlan {
        let mut tariff = unblock();
        tariff.traffic.reset = reset;
        tariff.traffic_addon.as_mut().unwrap().leftover = leftover;
        let snapshot = tariff.snapshot(tariff.period(PeriodId(31)).unwrap());
        let expire = Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap();
        let mut state = current(SubscriptionStatus::Active, expire);
        state.traffic_strategy = reset.strategy();
        state.traffic_limit_bytes = limit * GIB;
        state.traffic_used_bytes = used * GIB;
        plan(&PlanInput {
            grace: None,
            kind,
            snapshot: &snapshot,
            extra_devices: 0,
            held_devices: None,
            extra_traffic: extra * GIB,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &profile(),
            tag: None,
            manual_devices: None,
        })
        .unwrap()
    }

    #[test]
    fn сброс_при_оплате_сжигает_или_переносит_докупленный_остаток() {
        use crate::tariff::TrafficLeftover::{Burn, Carry};
        let renewal = PlanKind::Renewal;
        let burn = with_bought_traffic(renewal, TrafficReset::OnPayment, Burn, (350, 320, 50));
        assert_eq!(burn.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(burn.extra_traffic_bytes, 0);
        assert!(burn.reset_traffic);
        let carry = with_bought_traffic(renewal, TrafficReset::OnPayment, Carry, (350, 320, 50));
        assert_eq!(carry.target.traffic_limit_bytes, 330 * GIB);
        assert_eq!(carry.extra_traffic_bytes, 30 * GIB);
        assert!(!carry.keep_traffic_window);
        let spent = with_bought_traffic(renewal, TrafficReset::OnPayment, Carry, (350, 360, 50));
        assert_eq!(spent.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(spent.extra_traffic_bytes, 0);
    }

    #[test]
    fn без_сброса_сгорает_только_докупленный_остаток() {
        use crate::tariff::TrafficLeftover::{Burn, Carry};
        let renewal = PlanKind::Renewal;
        let burn = with_bought_traffic(renewal, TrafficReset::NoReset, Burn, (350, 100, 50));
        assert_eq!(burn.target.traffic_limit_bytes, 600 * GIB);
        assert_eq!(burn.extra_traffic_bytes, 0);
        assert!(!burn.reset_traffic);
        let carry = with_bought_traffic(renewal, TrafficReset::NoReset, Carry, (350, 100, 50));
        assert_eq!(carry.target.traffic_limit_bytes, 650 * GIB);
        assert_eq!(carry.extra_traffic_bytes, 50 * GIB);
    }

    #[test]
    fn при_сбросе_по_расписанию_докупленное_доживает_до_сброса() {
        use crate::tariff::TrafficLeftover::Burn;
        let p = with_bought_traffic(PlanKind::Renewal, TrafficReset::Month, Burn, (350, 100, 50));
        assert_eq!(p.target.traffic_limit_bytes, 350 * GIB);
        assert_eq!(p.extra_traffic_bytes, 50 * GIB);
        assert!(p.keep_traffic_window);
        assert!(!p.reset_traffic);
    }

    #[test]
    fn смена_тарифа_сжигает_докупленный_трафик() {
        use crate::tariff::TrafficLeftover::Carry;
        let p = with_bought_traffic(
            PlanKind::Change,
            TrafficReset::OnPayment,
            Carry,
            (350, 100, 50),
        );
        assert_eq!(p.target.traffic_limit_bytes, 300 * GIB);
        assert_eq!(p.extra_traffic_bytes, 0);
        assert!(!p.keep_traffic_window);
    }

    #[test]
    fn ручной_лимит_устройств_заменяет_тариф_при_продлении() {
        let tariff = standard();
        let snapshot = tariff.snapshot(tariff.period(PeriodId(11)).unwrap());
        let expire = Utc.with_ymd_and_hms(2027, 2, 14, 0, 0, 0).unwrap();
        let state = current(SubscriptionStatus::Active, expire);
        let owner = profile();
        let input = |manual_devices: Option<u32>, held_devices: Option<HeldDevices>| PlanInput {
            grace: None,
            extra_traffic: 0,
            kind: PlanKind::Renewal,
            snapshot: &snapshot,
            extra_devices: 1,
            held_devices,
            paid_at: paid_at(),
            current: Some(&state),
            keep_squads: &[],
            profile: &owner,
            tag: None,
            manual_devices,
        };
        assert_eq!(
            plan(&input(None, None)).unwrap().target.hwid_device_limit,
            Some(4)
        );
        let raised = plan(&input(Some(7), None)).unwrap();
        assert_eq!(raised.target.hwid_device_limit, Some(7));
        assert_eq!(raised.device_hold, None);
        let lowered = plan(&input(Some(1), None)).unwrap();
        assert_eq!(lowered.target.hwid_device_limit, Some(1));
        let held = plan(&input(
            Some(7),
            Some(HeldDevices {
                limit: 9,
                until: expire,
            }),
        ))
        .unwrap();
        assert_eq!(held.target.hwid_device_limit, Some(9));
        assert_eq!(held.device_hold.map(|h| h.limit_after), Some(7));
    }

    #[test]
    fn запас_автопродления_не_копится_и_не_съедает_оплаченное() {
        let at =
            |day: i64| Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap() + Duration::days(day);
        let first = graced_expire_at(PlanKind::New, None, at(0), 30, Some(Grace::first()));
        assert_eq!(first, at(32));
        let second = graced_expire_at(
            PlanKind::Renewal,
            Some(first),
            at(30),
            30,
            Some(Grace::renewal()),
        );
        assert_eq!(second, at(62));
        let third = graced_expire_at(
            PlanKind::Renewal,
            Some(second),
            at(61),
            30,
            Some(Grace::renewal()),
        );
        assert_eq!(third, at(93));
        let late = graced_expire_at(
            PlanKind::Renewal,
            Some(third),
            at(94),
            30,
            Some(Grace::renewal()),
        );
        assert_eq!(late, at(126));
        let with_left = graced_expire_at(
            PlanKind::Renewal,
            Some(at(20)),
            at(0),
            30,
            Some(Grace::first()),
        );
        assert_eq!(with_left, at(52));
        let after_left = graced_expire_at(
            PlanKind::Renewal,
            Some(with_left),
            at(30),
            30,
            Some(Grace::renewal()),
        );
        assert_eq!(after_left, at(82));
        let bought_more = graced_expire_at(
            PlanKind::Renewal,
            Some(at(92)),
            at(60),
            30,
            Some(Grace::renewal()),
        );
        assert_eq!(bought_more, at(122));
        let change = graced_expire_at(
            PlanKind::Change,
            Some(at(10)),
            at(0),
            30,
            Some(Grace::first()),
        );
        assert_eq!(change, at(32));
        assert_eq!(
            graced_expire_at(PlanKind::Renewal, Some(at(10)), at(0), 30, None),
            new_expire_at(PlanKind::Renewal, Some(at(10)), at(0), 30)
        );
    }

    fn noon(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
    }

    fn yearly(term: u32, first: DateTime<Utc>, left: Duration, charges: &[DateTime<Utc>]) {
        let year = Some(Cycle::Months(12));
        let buffer = Duration::hours(i64::from(RECURRING_GRACE_HOURS));
        let kind = if left > Duration::zero() {
            PlanKind::Renewal
        } else {
            PlanKind::New
        };
        let mut expire = graced_expire_at(
            kind,
            Some(first + left),
            first + Duration::minutes(3),
            term,
            Some(Grace::first().every(year, Some(first))),
        );
        let mut due = Cycle::Months(12).after(first);
        assert_eq!(expire, due + buffer + left, "срок {term}, первая оплата");
        for charge in charges {
            assert!(
                expire >= *charge,
                "срок {term}: доступ кончился раньше списания {charge}"
            );
            expire = graced_expire_at(
                PlanKind::Renewal,
                Some(expire),
                *charge + Duration::minutes(3),
                term,
                Some(Grace::renewal().every(year, Some(*charge))),
            );
            due = Cycle::Months(12).after(*charge);
            assert_eq!(
                expire,
                due + buffer + left,
                "срок {term}, списание {charge}"
            );
        }
    }

    #[test]
    fn годовая_подписка_даёт_доступ_до_следующего_списания_без_перерыва_и_без_накопления() {
        let charges = [noon(2027, 3, 10), noon(2028, 3, 10), noon(2029, 3, 10)];
        assert_eq!(charges[1] - charges[0], Duration::days(366));
        assert_eq!(charges[2] - charges[1], Duration::days(365));
        yearly(360, noon(2026, 3, 10), Duration::zero(), &charges);
        yearly(365, noon(2026, 3, 10), Duration::zero(), &charges);
        yearly(366, noon(2026, 3, 10), Duration::zero(), &charges);
        yearly(360, noon(2026, 3, 10), Duration::days(10), &charges);
        let late = [
            noon(2027, 3, 11) + Duration::hours(8),
            noon(2028, 3, 11) + Duration::hours(8),
        ];
        yearly(360, noon(2026, 3, 10), Duration::zero(), &late);
    }

    #[test]
    fn годовой_срок_365_дней_в_обычный_год_как_раньше() {
        let paid = noon(2026, 3, 10);
        let buffer = Duration::hours(i64::from(RECURRING_GRACE_HOURS));
        assert_eq!(
            graced_expire_at(
                PlanKind::New,
                None,
                paid,
                365,
                Some(Grace::first().every(Some(Cycle::Months(12)), Some(paid)))
            ),
            graced_expire_at(PlanKind::New, None, paid, 365, Some(Grace::first()))
        );
        assert_eq!(
            graced_expire_at(PlanKind::New, None, paid, 365, Some(Grace::first())),
            paid + buffer + Duration::days(365)
        );
        let leap = noon(2027, 3, 10);
        assert_eq!(
            graced_expire_at(
                PlanKind::New,
                None,
                leap,
                365,
                Some(Grace::first().every(Some(Cycle::Months(12)), Some(leap)))
            ),
            leap + buffer + Duration::days(366)
        );
    }

    #[test]
    fn цикл_по_месяцам_берёт_то_же_число_или_конец_месяца() {
        assert_eq!(Cycle::Months(1).after(noon(2027, 1, 31)), noon(2027, 2, 28));
        assert_eq!(Cycle::Months(1).after(noon(2028, 1, 31)), noon(2028, 2, 29));
        assert_eq!(
            Cycle::Months(12).after(noon(2028, 2, 29)),
            noon(2029, 2, 28)
        );
        assert_eq!(Cycle::Days(30).after(noon(2027, 1, 31)), noon(2027, 3, 2));
        let charged = noon(2027, 6, 1);
        let late = graced_expire_at(
            PlanKind::Renewal,
            Some(noon(2027, 6, 3)),
            charged + Duration::days(3),
            30,
            Some(Grace::renewal().every(Some(Cycle::Days(30)), Some(charged))),
        );
        assert_eq!(
            late,
            noon(2027, 7, 1) + Duration::hours(48),
            "срок считается от списания, а не от того, когда панель о нём узнала"
        );
    }
}
