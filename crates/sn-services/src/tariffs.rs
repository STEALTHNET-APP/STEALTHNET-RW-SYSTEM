use crate::Actor;
use crate::error::{ServiceError, ServiceResult};
use crate::services::Services;
use serde::{Deserialize, Serialize};
use sn_domain::DomainError;
use sn_domain::format::plural_ru;
use sn_domain::ids::{AdminId, CategoryId, PeriodId, RemnaUserId, SquadId, TariffId};
use sn_domain::money::{Currency, Stars};
use sn_domain::tariff::{
    Category, CurrencyPrices, DeviceDiscount, DevicePolicy, Tariff, TariffChannels, TariffPeriod,
    TrafficAddon, check_device_discounts, check_subscription_limit,
};
use sn_domain::traffic::TrafficPolicy;
use sn_remnawave::{BulkSquadsRequest, BulkUpdateRequest, BulkUserFields};
use sn_store::{Db, NewCategory, NewPeriod, NewTariff, TariffUsage};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const MAX_NAME_CHARS: usize = 100;
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
pub const MAX_PERIOD_DAYS: u32 = 3_650;
pub const MAX_DEVICES: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PeriodInput {
    pub days: u32,
    #[serde(default)]
    pub prices: CurrencyPrices,
    #[serde(default)]
    pub stars: Option<Stars>,
    #[serde(default)]
    pub sort: i32,
    #[serde(default = "enabled")]
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TariffInput {
    pub category_id: CategoryId,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "enabled")]
    pub on_sale: bool,
    #[serde(default)]
    pub sort: i32,
    pub devices: DevicePolicy,
    #[serde(default)]
    pub traffic: TrafficPolicy,
    #[serde(default)]
    pub traffic_addon: Option<TrafficAddon>,
    pub squads: Vec<SquadId>,
    #[serde(default)]
    pub external_squad: Option<SquadId>,
    pub periods: Vec<PeriodInput>,
    #[serde(default)]
    pub device_discounts: Vec<DeviceDiscount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CategoryInput {
    pub name: String,
    #[serde(default)]
    pub sort: i32,
    #[serde(default = "enabled")]
    pub visible: bool,
    #[serde(default)]
    pub subscription_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffStats {
    pub tariff_id: TariffId,
    pub sales_30d: i64,
    pub channels: TariffChannels,
}

pub const IN_USE: &str = "tariff.in_use";
pub const HOLDERS_SHOWN: i64 = 20;

fn count(n: i64, one: &str, few: &str, many: &str) -> String {
    let n = u64::try_from(n).unwrap_or_default();
    format!("{n} {}", plural_ru(n, one, few, many))
}

pub fn held(usage: &TariffUsage) -> String {
    match (usage.active, usage.ended) {
        (active, 0) => count(
            active,
            "действующая подписка",
            "действующие подписки",
            "действующих подписок",
        ),
        (0, ended) => count(
            ended,
            "закончившаяся подписка",
            "закончившиеся подписки",
            "закончившихся подписок",
        ),
        (_, ended) => format!(
            "{}, из них {} {ended}",
            count(usage.total(), "подписка", "подписки", "подписок"),
            plural_ru(
                u64::try_from(ended).unwrap_or_default(),
                "закончилась",
                "закончились",
                "закончились"
            )
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TariffsView {
    pub currency: Currency,
    pub categories: Vec<Category>,
    pub tariffs: Vec<Tariff>,
    pub stats: Vec<TariffStats>,
    pub renewals: Vec<crate::renewals::BoundTerm>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    pub users: usize,
    pub failed: usize,
    pub squads: bool,
    pub traffic: bool,
    pub devices: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SquadBrief {
    pub uuid: SquadId,
    pub name: String,
    pub nodes: Option<usize>,
}

const APPLY_BATCH: usize = 500;

const fn enabled() -> bool {
    true
}

fn invalid(field: &str, reason: impl Into<String>) -> ServiceError {
    ServiceError::rejected("tariff.invalid", format!("{field}: {}", reason.into()))
}

fn check_prices(field: &str, prices: &CurrencyPrices) -> ServiceResult<()> {
    for (currency, value) in [(Currency::Usd, prices.usd), (Currency::Rub, prices.rub)] {
        let Some(value) = value else {
            continue;
        };
        if value < 0 {
            return Err(invalid(field, "цена не может быть отрицательной"));
        }
        if currency == Currency::Rub && value % Currency::Rub.rounding_step() != 0 {
            return Err(invalid(field, "цена в рублях задаётся целыми рублями"));
        }
    }
    Ok(())
}

fn invalid_from(field: &str, err: DomainError) -> ServiceError {
    match err {
        DomainError::InvalidValue { reason, .. } => invalid(field, reason),
        other => other.into(),
    }
}

fn check_stars(field: &str, stars: Option<Stars>) -> ServiceResult<()> {
    if stars.is_some_and(|s| s.0 <= 0) {
        return Err(invalid(field, "цена в звёздах должна быть больше нуля"));
    }
    Ok(())
}

impl TariffInput {
    pub fn validate(&self) -> ServiceResult<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(invalid("Название", "от 1 до 100 символов"));
        }
        if self
            .description
            .as_deref()
            .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_CHARS)
        {
            return Err(invalid("Описание", "не длиннее 2000 символов"));
        }
        if self.periods.is_empty() {
            return Err(invalid("Сроки", "нужен хотя бы один срок"));
        }
        let mut days: Vec<u32> = Vec::with_capacity(self.periods.len());
        for period in &self.periods {
            if period.days == 0 || period.days > MAX_PERIOD_DAYS {
                return Err(invalid("Срок", "от 1 до 3650 дней"));
            }
            if days.contains(&period.days) {
                return Err(invalid(
                    "Срок",
                    format!("срок {} дней указан дважды", period.days),
                ));
            }
            days.push(period.days);
            check_prices("Цена", &period.prices)?;
            check_stars("Цена в звёздах", period.stars)?;
        }
        if self.devices.included == 0 || self.devices.included > MAX_DEVICES {
            return Err(invalid("Устройств в тарифе", "от 1 до 100"));
        }
        if let Some(extra) = &self.devices.extra {
            if extra.max == 0 || extra.max > MAX_DEVICES {
                return Err(invalid("Доп. устройств не больше", "от 1 до 100"));
            }
            check_prices("Цена за устройство в месяц", &extra.monthly)?;
            check_stars("Цена устройства в звёздах", extra.monthly_stars)?;
            check_device_discounts(&self.device_discounts, extra.max)
                .map_err(|err| invalid_from("Скидка за доп. устройства", err))?;
        }
        self.traffic.validate()?;
        if let Some(addon) = &self.traffic_addon {
            for pack in &addon.packs {
                check_prices("Цена пакета трафика", &pack.prices)?;
                check_stars("Цена пакета в звёздах", pack.stars)?;
            }
        }
        if self.squads.is_empty() {
            return Err(invalid(
                "Сквады",
                "выберите хотя бы один сквад, иначе у клиентов не будет серверов",
            ));
        }
        let mut squads = self.squads.clone();
        squads.sort();
        squads.dedup();
        if squads.len() != self.squads.len() {
            return Err(invalid("Сквады", "сквад указан дважды"));
        }
        self.preview().validate()?;
        Ok(())
    }

    fn preview(&self) -> Tariff {
        Tariff {
            traffic_addon: self.addon(),
            id: TariffId(0),
            category_id: self.category_id,
            name: self.name.trim().to_string(),
            description: self.description.clone(),
            on_sale: self.on_sale,
            sort: self.sort,
            periods: self
                .periods
                .iter()
                .map(|p| TariffPeriod {
                    id: PeriodId(0),
                    days: p.days,
                    prices: p.prices,
                    stars: p.stars,
                    sort: p.sort,
                    active: p.active,
                })
                .collect(),
            devices: self.devices.clone(),
            traffic: self.traffic,
            squads: self.squads.clone(),
            external_squad: self.external_squad,
            device_discounts: self.discounts(),
        }
    }

    pub fn discounts(&self) -> Vec<DeviceDiscount> {
        match &self.devices.extra {
            Some(extra) => {
                check_device_discounts(&self.device_discounts, extra.max).unwrap_or_default()
            }
            None => Vec::new(),
        }
    }

    fn addon(&self) -> Option<TrafficAddon> {
        self.traffic_addon
            .clone()
            .filter(|addon| !addon.packs.is_empty() && self.traffic.is_limited())
    }

    pub fn to_new(&self) -> NewTariff {
        NewTariff {
            traffic_addon: self.addon(),
            category_id: self.category_id,
            name: self.name.trim().to_string(),
            description: self
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string),
            on_sale: self.on_sale,
            sort: self.sort,
            devices: self.devices.clone(),
            traffic: self.traffic,
            squads: self.squads.clone(),
            external_squad: self.external_squad,
            periods: self
                .periods
                .iter()
                .map(|p| NewPeriod {
                    days: p.days,
                    prices: p.prices,
                    stars: p.stars,
                    sort: p.sort,
                    active: p.active,
                })
                .collect(),
        }
    }
}

impl CategoryInput {
    pub fn to_new(&self) -> ServiceResult<NewCategory> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(invalid("Категория", "название от 1 до 60 символов"));
        }
        check_subscription_limit(self.subscription_limit)
            .map_err(|err| invalid_from("Подписок этой категории у клиента", err))?;
        Ok(NewCategory {
            name: name.to_string(),
            sort: self.sort,
            visible: self.visible,
        })
    }
}

pub struct TariffAdmin<'a> {
    services: &'a Services,
}

impl<'a> TariffAdmin<'a> {
    pub fn new(services: &'a Services) -> Self {
        TariffAdmin { services }
    }

    pub async fn list(&self) -> ServiceResult<TariffsView> {
        let mut conn = self.services.store().conn().await?;
        let tariffs = conn.tariffs().list(true).await?;
        let ids: Vec<TariffId> = tariffs.iter().map(|t| t.id).collect();
        let channels = conn.tariffs().channels(&ids).await?;
        let since = self.services.now() - chrono::Duration::days(30);
        let sales: HashMap<TariffId, i64> = conn
            .tariffs()
            .sales_since(since)
            .await?
            .into_iter()
            .collect();
        let stats = ids
            .iter()
            .map(|id| TariffStats {
                tariff_id: *id,
                sales_30d: sales.get(id).copied().unwrap_or(0),
                channels: channels.get(id).copied().unwrap_or_default(),
            })
            .collect();
        Ok(TariffsView {
            currency: self.services.settings().currency,
            categories: conn.tariffs().categories(true).await?,
            tariffs,
            stats,
            renewals: crate::renewals::bound_terms(self.services),
        })
    }

    pub async fn set_channels(
        &self,
        id: TariffId,
        channels: TariffChannels,
        admin: AdminId,
    ) -> ServiceResult<TariffChannels> {
        if !channels.any() {
            return Err(invalid(
                "Где показывать",
                "оставьте хотя бы одно место или снимите тариф с продажи",
            ));
        }
        let mut tx = self.services.store().begin().await?;
        tx.tariffs().get(id).await?;
        tx.tariffs().set_channels(id, channels).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.channels",
                "tariff",
                Some(id.0),
                serde_json::json!({ "channels": channels }),
            ))
            .await?;
        tx.commit().await?;
        Ok(channels)
    }

    pub async fn usage(&self, id: TariffId, people: bool) -> ServiceResult<TariffUsage> {
        let mut conn = self.services.store().conn().await?;
        conn.tariffs().get(id).await?;
        let shown = if people { HOLDERS_SHOWN } else { 0 };
        Ok(conn.tariffs().usage(id, self.services.now(), shown).await?)
    }

    pub async fn delete(&self, id: TariffId, admin: AdminId) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let tariff = tx.tariffs().get(id).await?;
        let usage = tx.tariffs().usage(id, self.services.now(), 0).await?;
        if usage.total() > 0 {
            return Err(ServiceError::rejected(
                IN_USE,
                format!(
                    "На тарифе {}. Удалить его можно, когда подписок на нём не останется: переведите клиентов на другой тариф или удалите их подписки в карточке клиента",
                    held(&usage)
                ),
            ));
        }
        tx.tariffs().soft_delete(id).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.delete",
                "tariff",
                Some(id.0),
                serde_json::json!({ "name": tariff.name }),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_category(&self, id: CategoryId, admin: AdminId) -> ServiceResult<()> {
        let mut tx = self.services.store().begin().await?;
        let used = tx.tariffs().category_tariffs(id).await?;
        if used > 0 {
            return Err(ServiceError::rejected(
                "category.in_use",
                "в категории есть тарифы: перенесите или удалите их",
            ));
        }
        tx.tariffs().delete_category(id).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "category.delete",
                "category",
                Some(id.0),
                serde_json::json!({}),
            ))
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn squads(&self) -> ServiceResult<Vec<SquadBrief>> {
        let remna = self.services.remna()?;
        let squads = remna.internal_squads().await?.internal_squads;
        let mut counts = Vec::with_capacity(squads.len());
        for squad in &squads {
            counts.push(
                remna
                    .squad_accessible_nodes(squad.uuid)
                    .await
                    .ok()
                    .map(|nodes| nodes.accessible_nodes.len()),
            );
        }
        let mut out: Vec<(i64, SquadBrief)> = squads
            .into_iter()
            .zip(counts)
            .map(|(squad, nodes)| {
                (
                    squad.view_position,
                    SquadBrief {
                        uuid: SquadId(squad.uuid),
                        name: squad.name,
                        nodes,
                    },
                )
            })
            .collect();
        out.sort_by_key(|(position, _)| *position);
        Ok(out.into_iter().map(|(_, squad)| squad).collect())
    }

    pub async fn apply_to_subscribers(
        &self,
        before: &Tariff,
        after: &Tariff,
    ) -> ServiceResult<ApplyReport> {
        let mut old_squads = before.squads.clone();
        old_squads.sort();
        let mut new_squads = after.squads.clone();
        new_squads.sort();
        let squads = old_squads != new_squads;
        let traffic = before.traffic != after.traffic;
        let devices = before.devices.included != after.devices.included;
        let mut report = ApplyReport {
            users: 0,
            failed: 0,
            squads,
            traffic,
            devices,
        };
        if !(squads || traffic || devices) {
            return Ok(report);
        }
        let users = {
            let mut conn = self.services.store().conn().await?;
            conn.subscriptions().linked_for_tariff(after.id).await?
        };
        report.users = users.len();
        if users.is_empty() {
            return Ok(report);
        }
        let remna = self.services.remna()?;
        let mut failed: HashSet<RemnaUserId> = HashSet::new();
        if squads {
            let target: Vec<uuid::Uuid> = after.squads.iter().map(|s| s.0).collect();
            for chunk in users.chunks(APPLY_BATCH) {
                let ids: Vec<RemnaUserId> = chunk.iter().map(|(id, _, _)| *id).collect();
                let request = BulkSquadsRequest {
                    user_ids: ids.clone(),
                    active_internal_squads: target.clone(),
                };
                if let Err(err) = remna.bulk_update_squads(&request).await {
                    tracing::warn!(tariff = %after.id, error = %err, "сквады подписчиков не обновились");
                    failed.extend(ids);
                }
            }
        }
        if traffic || devices {
            let mut groups: BTreeMap<u32, Vec<RemnaUserId>> = BTreeMap::new();
            for (user, extra, manual) in &users {
                let total = manual.unwrap_or_else(|| after.devices.included.saturating_add(*extra));
                groups.entry(total).or_default().push(*user);
            }
            for (total, ids) in groups {
                for chunk in ids.chunks(APPLY_BATCH) {
                    let fields = BulkUserFields {
                        status: None,
                        traffic_limit_bytes: traffic.then(|| {
                            i64::try_from(after.traffic.remna_limit_bytes()).unwrap_or(i64::MAX)
                        }),
                        traffic_limit_strategy: traffic.then(|| after.traffic.reset.strategy()),
                        expire_at: None,
                        hwid_device_limit: devices.then(|| (total > 0).then_some(i64::from(total))),
                        tag: None,
                    };
                    let request = BulkUpdateRequest {
                        user_ids: chunk.to_vec(),
                        fields,
                    };
                    if let Err(err) = remna.bulk_update_users(&request).await {
                        tracing::warn!(tariff = %after.id, error = %err, "лимиты подписчиков не обновились");
                        failed.extend(chunk.iter().copied());
                    }
                }
            }
        }
        report.failed = failed.len();
        Ok(report)
    }

    pub async fn get(&self, id: TariffId) -> ServiceResult<Tariff> {
        let mut conn = self.services.store().conn().await?;
        Ok(conn.tariffs().get(id).await?)
    }

    async fn check_references(&self, db: &mut impl Db, input: &TariffInput) -> ServiceResult<()> {
        if !db
            .tariffs()
            .categories(true)
            .await?
            .iter()
            .any(|c| c.id == input.category_id)
        {
            return Err(ServiceError::NotFound("категория"));
        }
        let Ok(remna) = self.services.remna() else {
            return Ok(());
        };
        match remna.internal_squads().await {
            Ok(known) => {
                let missing: Vec<String> = input
                    .squads
                    .iter()
                    .filter(|s| !known.internal_squads.iter().any(|k| k.uuid == s.0))
                    .map(|s| s.to_string())
                    .collect();
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(invalid(
                        "Сквады",
                        format!("в Remnawave нет сквадов {}", missing.join(", ")),
                    ))
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "сквады тарифа не сверены с Remnawave");
                Ok(())
            }
        }
    }

    pub async fn create(&self, input: &TariffInput, admin: AdminId) -> ServiceResult<Tariff> {
        input.validate()?;
        let mut tx = self.services.store().begin().await?;
        self.check_references(&mut tx, input).await?;
        let created = tx.tariffs().create(&input.to_new()).await?;
        tx.tariffs()
            .set_device_discounts(created.id, &input.discounts())
            .await?;
        let tariff = tx.tariffs().get(created.id).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.create",
                "tariff",
                Some(tariff.id.0),
                serde_json::json!({ "name": tariff.name }),
            ))
            .await?;
        tx.commit().await?;
        Ok(tariff)
    }

    pub async fn update(
        &self,
        id: TariffId,
        input: &TariffInput,
        admin: AdminId,
    ) -> ServiceResult<Tariff> {
        input.validate()?;
        let mut tx = self.services.store().begin().await?;
        tx.tariffs().get(id).await?;
        self.check_references(&mut tx, input).await?;
        tx.tariffs().update(id, &input.to_new()).await?;
        tx.tariffs()
            .set_device_discounts(id, &input.discounts())
            .await?;
        let tariff = tx.tariffs().get(id).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.update",
                "tariff",
                Some(id.0),
                serde_json::json!({ "name": tariff.name }),
            ))
            .await?;
        tx.commit().await?;
        Ok(tariff)
    }

    pub async fn update_and_apply(
        &self,
        id: TariffId,
        input: &TariffInput,
        admin: AdminId,
        apply: bool,
    ) -> ServiceResult<(Tariff, Option<ApplyReport>)> {
        let before = self.get(id).await?;
        let tariff = self.update(id, input, admin).await?;
        if !apply {
            return Ok((tariff, None));
        }
        let report = self.apply_to_subscribers(&before, &tariff).await?;
        let mut tx = self.services.store().begin().await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.apply",
                "tariff",
                Some(id.0),
                serde_json::json!({ "report": report }),
            ))
            .await?;
        tx.commit().await?;
        Ok((tariff, Some(report)))
    }

    pub async fn set_on_sale(
        &self,
        id: TariffId,
        on_sale: bool,
        admin: AdminId,
    ) -> ServiceResult<Tariff> {
        let mut tx = self.services.store().begin().await?;
        tx.tariffs().set_on_sale(id, on_sale).await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "tariff.on_sale",
                "tariff",
                Some(id.0),
                serde_json::json!({ "on_sale": on_sale }),
            ))
            .await?;
        let tariff = tx.tariffs().get(id).await?;
        tx.commit().await?;
        Ok(tariff)
    }

    pub async fn create_category(
        &self,
        input: &CategoryInput,
        admin: AdminId,
    ) -> ServiceResult<Category> {
        let category = input.to_new()?;
        let mut tx = self.services.store().begin().await?;
        let created = tx.tariffs().create_category(&category).await?;
        let created = tx
            .tariffs()
            .set_subscription_limit(created.id, input.subscription_limit)
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "category.create",
                "category",
                Some(created.id.0),
                serde_json::json!({
                    "name": created.name,
                    "subscription_limit": created.subscription_limit,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(created)
    }

    pub async fn update_category(
        &self,
        id: CategoryId,
        input: &CategoryInput,
        admin: AdminId,
    ) -> ServiceResult<Category> {
        let category = input.to_new()?;
        let mut tx = self.services.store().begin().await?;
        tx.tariffs().update_category(id, &category).await?;
        let updated = tx
            .tariffs()
            .set_subscription_limit(id, input.subscription_limit)
            .await?;
        tx.audit()
            .record(&Actor::Admin(admin).audit(
                "category.update",
                "category",
                Some(id.0),
                serde_json::json!({
                    "name": updated.name,
                    "visible": updated.visible,
                    "subscription_limit": updated.subscription_limit,
                }),
            ))
            .await?;
        tx.commit().await?;
        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sn_domain::tariff::ExtraDevices;
    use uuid::Uuid;

    #[test]
    fn подписки_на_тарифе_названы_с_правильными_окончаниями() {
        let usage = |active, ended| TariffUsage {
            active,
            ended,
            holders: Vec::new(),
        };
        for (active, ended, text) in [
            (1, 0, "1 действующая подписка"),
            (2, 0, "2 действующие подписки"),
            (5, 0, "5 действующих подписок"),
            (11, 0, "11 действующих подписок"),
            (21, 0, "21 действующая подписка"),
            (0, 1, "1 закончившаяся подписка"),
            (0, 3, "3 закончившиеся подписки"),
            (2, 1, "3 подписки, из них закончилась 1"),
            (1, 4, "5 подписок, из них закончились 4"),
        ] {
            assert_eq!(held(&usage(active, ended)), text);
        }
    }

    fn input() -> TariffInput {
        TariffInput {
            traffic_addon: None,
            category_id: CategoryId(1),
            name: " Стандарт ".into(),
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
                    monthly_stars: Some(Stars(50)),
                }),
            },
            traffic: TrafficPolicy::UNLIMITED,
            squads: vec![SquadId(Uuid::from_u128(1))],
            external_squad: None,
            device_discounts: Vec::new(),
            periods: vec![PeriodInput {
                days: 30,
                prices: CurrencyPrices {
                    usd: Some(299),
                    rub: Some(19_900),
                },
                stars: Some(Stars(150)),
                sort: 0,
                active: true,
            }],
        }
    }

    #[test]
    fn тариф_из_формы_проверяется_до_записи() {
        assert!(input().validate().is_ok());
        assert_eq!(input().to_new().name, "Стандарт");
        let mut kopecks = input();
        kopecks.periods[0].prices.rub = Some(19_950);
        assert!(kopecks.validate().is_err());
        let mut twice = input();
        twice.periods.push(twice.periods[0].clone());
        assert!(twice.validate().is_err());
        let mut no_squads = input();
        no_squads.squads.clear();
        assert!(no_squads.validate().is_err());
        let mut no_devices = input();
        no_devices.devices.included = 0;
        assert!(no_devices.validate().is_err());
        let mut free_stars = input();
        free_stars.periods[0].stars = Some(Stars(0));
        assert!(free_stars.validate().is_err());
    }

    #[test]
    fn скидка_за_устройства_из_формы_проверяется_и_сортируется() {
        let step = |from, percent| DeviceDiscount { from, percent };
        let mut ladder = input();
        ladder.device_discounts = vec![step(2, 20), step(1, 10)];
        assert!(ladder.validate().is_ok());
        assert_eq!(ladder.discounts(), vec![step(1, 10), step(2, 20)]);
        let mut too_many = input();
        too_many.device_discounts = vec![step(3, 10)];
        let err = too_many.validate().unwrap_err().to_string();
        assert!(err.contains("Скидка за доп. устройства"), "{err}");
        let mut falling = input();
        falling.device_discounts = vec![step(1, 20), step(2, 10)];
        assert!(falling.validate().is_err());
        let mut not_sold = input();
        not_sold.devices.extra = None;
        not_sold.device_discounts = vec![step(1, 10)];
        assert!(not_sold.validate().is_ok());
        assert!(not_sold.discounts().is_empty());
    }

    #[test]
    fn лимит_подписок_категории_из_формы() {
        let category = |limit| CategoryInput {
            name: "С обходом".into(),
            sort: 0,
            visible: true,
            subscription_limit: limit,
        };
        assert!(category(None).to_new().is_ok());
        assert!(category(Some(1)).to_new().is_ok());
        assert!(category(Some(0)).to_new().is_err());
        assert!(category(Some(101)).to_new().is_err());
    }
}
