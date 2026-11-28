use crate::DomainError;
use crate::access::ClientChannel;
use crate::bot_content::{normalize_static_markup, visible_len};
use crate::ids::{CategoryId, PeriodId, SquadId, TariffId};
use crate::money::{Currency, Money, Stars, div_round_half_up};
use crate::promotion::{PromotionTerms, discounted, discounted_stars};
use crate::traffic::{GIB, TrafficPolicy};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CurrencyPrices {
    pub usd: Option<i64>,
    pub rub: Option<i64>,
}

impl CurrencyPrices {
    pub const fn get(&self, currency: Currency) -> Option<Money> {
        let minor = match currency {
            Currency::Usd => self.usd,
            Currency::Rub => self.rub,
        };
        match minor {
            Some(minor) => Some(Money::new(minor, currency)),
            None => None,
        }
    }

    pub fn set(&mut self, currency: Currency, minor: Option<i64>) {
        match currency {
            Currency::Usd => self.usd = minor,
            Currency::Rub => self.rub = minor,
        }
    }

    pub fn validate(&self, field: &'static str) -> Result<(), DomainError> {
        for value in [self.usd, self.rub].into_iter().flatten() {
            if value < 0 {
                return Err(DomainError::invalid(
                    field,
                    "цена не может быть отрицательной",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: CategoryId,
    pub name: String,
    pub sort: i32,
    pub visible: bool,
    #[serde(default)]
    pub subscription_limit: Option<u32>,
}

pub const MAX_SUBSCRIPTION_LIMIT: u32 = 100;

impl Category {
    pub fn is_full(&self, multi: bool, held: usize) -> bool {
        subscriptions_full(multi, self.subscription_limit, held)
    }
}

pub fn subscriptions_full(multi: bool, limit: Option<u32>, held: usize) -> bool {
    multi && limit.is_some_and(|limit| held >= limit as usize)
}

pub fn check_subscription_limit(limit: Option<u32>) -> Result<(), DomainError> {
    match limit {
        Some(limit) if limit == 0 || limit > MAX_SUBSCRIPTION_LIMIT => Err(DomainError::invalid(
            "subscription_limit",
            "подписок этой категории у клиента — от 1 до 100 или без ограничений",
        )),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TariffPeriod {
    pub id: PeriodId,
    pub days: u32,
    pub prices: CurrencyPrices,
    pub stars: Option<Stars>,
    pub sort: i32,
    pub active: bool,
}

impl TariffPeriod {
    pub fn months(&self) -> u32 {
        months_for_days(self.days)
    }

    pub const fn price(&self, currency: Currency) -> Option<Money> {
        self.prices.get(currency)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraDevices {
    pub max: u32,
    pub monthly: CurrencyPrices,
    pub monthly_stars: Option<Stars>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePolicy {
    pub included: u32,
    pub extra: Option<ExtraDevices>,
}

pub const MAX_DEVICE_DISCOUNTS: usize = 10;
pub const MAX_DEVICE_DISCOUNT_PERCENT: u8 = 99;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceDiscount {
    pub from: u32,
    pub percent: u8,
}

pub fn device_discount(discounts: &[DeviceDiscount], extra: u32) -> Option<DeviceDiscount> {
    discounts
        .iter()
        .filter(|d| d.from > 0 && d.from <= extra && d.percent > 0)
        .max_by_key(|d| d.from)
        .copied()
}

pub fn check_device_discounts(
    discounts: &[DeviceDiscount],
    max_extra: u32,
) -> Result<Vec<DeviceDiscount>, DomainError> {
    const FIELD: &str = "device_discounts";
    if discounts.len() > MAX_DEVICE_DISCOUNTS {
        return Err(DomainError::invalid(
            FIELD,
            "ступеней скидки за устройства может быть не больше десяти",
        ));
    }
    let mut sorted = discounts.to_vec();
    sorted.sort_by_key(|d| d.from);
    for (index, step) in sorted.iter().enumerate() {
        if step.from == 0 {
            return Err(DomainError::invalid(
                FIELD,
                "скидка начинается хотя бы с одного доп. устройства",
            ));
        }
        if step.from > max_extra {
            return Err(DomainError::invalid(
                FIELD,
                format!(
                    "скидка от {} доп. устройств не сработает: докупить можно не больше {max_extra}",
                    step.from
                ),
            ));
        }
        if step.percent == 0 || step.percent > MAX_DEVICE_DISCOUNT_PERCENT {
            return Err(DomainError::invalid(FIELD, "скидка — от 1 до 99 %"));
        }
        if let Some(previous) = index.checked_sub(1).map(|i| sorted[i]) {
            if previous.from == step.from {
                return Err(DomainError::invalid(
                    FIELD,
                    format!("две ступени от {} доп. устройств: оставьте одну", step.from),
                ));
            }
            if previous.percent >= step.percent {
                return Err(DomainError::invalid(
                    FIELD,
                    format!(
                        "скидка от {} доп. устройств должна быть больше, чем от {}",
                        step.from, previous.from
                    ),
                ));
            }
        }
    }
    Ok(sorted)
}

impl DevicePolicy {
    pub fn max_extra(&self) -> u32 {
        self.extra.as_ref().map_or(0, |e| e.max)
    }

    pub fn max_total(&self) -> u32 {
        self.included.saturating_add(self.max_extra())
    }

    pub fn check_extra(&self, wanted: u32, selling_enabled: bool) -> Result<(), DomainError> {
        if wanted == 0 {
            return Ok(());
        }
        if !selling_enabled || self.extra.is_none() {
            return Err(DomainError::ExtraDevicesNotSold);
        }
        let max = self.max_extra();
        if wanted > max {
            return Err(DomainError::TooManyExtraDevices { max });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficLeftover {
    #[default]
    Burn,
    Carry,
}

impl TrafficLeftover {
    pub const fn as_str(self) -> &'static str {
        match self {
            TrafficLeftover::Burn => "burn",
            TrafficLeftover::Carry => "carry",
        }
    }

    pub const fn carries(self) -> bool {
        matches!(self, TrafficLeftover::Carry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficPack {
    pub gb: u32,
    pub prices: CurrencyPrices,
    #[serde(default)]
    pub stars: Option<Stars>,
}

impl TrafficPack {
    pub const fn bytes(&self) -> u64 {
        self.gb as u64 * GIB
    }

    pub const fn price(&self, currency: Currency) -> Option<Money> {
        self.prices.get(currency)
    }
}

pub const MAX_TRAFFIC_PACKS: usize = 8;
pub const MAX_PACK_GB: u32 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrafficAddon {
    pub packs: Vec<TrafficPack>,
    #[serde(default)]
    pub leftover: TrafficLeftover,
}

impl TrafficAddon {
    pub fn offered(&self, currency: Currency) -> Vec<&TrafficPack> {
        let mut packs: Vec<&TrafficPack> = self
            .packs
            .iter()
            .filter(|p| p.price(currency).is_some())
            .collect();
        packs.sort_by_key(|p| p.gb);
        packs
    }

    pub fn pack(&self, gb: u32) -> Option<&TrafficPack> {
        self.packs.iter().find(|p| p.gb == gb)
    }

    pub fn validate(&self, limited: bool) -> Result<(), DomainError> {
        if self.packs.is_empty() {
            return Ok(());
        }
        if !limited {
            return Err(DomainError::invalid(
                "traffic_addon",
                "докупка трафика есть только у тарифов с лимитом трафика",
            ));
        }
        if self.packs.len() > MAX_TRAFFIC_PACKS {
            return Err(DomainError::invalid(
                "traffic_addon",
                "пакетов трафика может быть не больше восьми",
            ));
        }
        for (index, pack) in self.packs.iter().enumerate() {
            if pack.gb == 0 || pack.gb > MAX_PACK_GB {
                return Err(DomainError::invalid(
                    "traffic_addon",
                    "размер пакета — от 1 до 100 000 ГБ",
                ));
            }
            if self.packs[..index].iter().any(|p| p.gb == pack.gb) {
                return Err(DomainError::invalid(
                    "traffic_addon",
                    "два пакета одного размера: оставьте один",
                ));
            }
            pack.prices.validate("traffic_pack_price")?;
            if matches!(pack.stars, Some(Stars(n)) if n <= 0) {
                return Err(DomainError::invalid(
                    "traffic_pack_stars",
                    "цена в звёздах должна быть больше нуля",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tariff {
    pub id: TariffId,
    pub category_id: CategoryId,
    pub name: String,
    pub description: Option<String>,
    pub on_sale: bool,
    pub sort: i32,
    pub periods: Vec<TariffPeriod>,
    pub devices: DevicePolicy,
    pub traffic: TrafficPolicy,
    #[serde(default)]
    pub traffic_addon: Option<TrafficAddon>,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
    #[serde(default)]
    pub device_discounts: Vec<DeviceDiscount>,
}

impl Tariff {
    pub fn device_discount(&self, extra: u32) -> Option<DeviceDiscount> {
        self.devices.extra.as_ref()?;
        device_discount(&self.device_discounts, extra)
    }

    pub fn period(&self, id: PeriodId) -> Option<&TariffPeriod> {
        self.periods.iter().find(|p| p.id == id)
    }

    pub fn sellable_period(
        &self,
        id: PeriodId,
        currency: Currency,
    ) -> Result<&TariffPeriod, DomainError> {
        if !self.on_sale {
            return Err(DomainError::TariffNotOnSale(self.id));
        }
        let period = self.period(id).ok_or(DomainError::PeriodNotFound(id))?;
        if !period.active {
            return Err(DomainError::PeriodInactive(id));
        }
        if period.price(currency).is_none() {
            return Err(DomainError::NoPriceInCurrency(currency.code()));
        }
        Ok(period)
    }

    pub fn offered_periods(&self, currency: Currency) -> Vec<&TariffPeriod> {
        let mut periods: Vec<&TariffPeriod> = self
            .periods
            .iter()
            .filter(|p| p.active && p.price(currency).is_some())
            .collect();
        periods.sort_by_key(|p| (p.sort, p.days));
        periods
    }

    pub fn is_offered(&self, currency: Currency) -> bool {
        self.on_sale && !self.offered_periods(currency).is_empty()
    }

    pub fn snapshot(&self, period: &TariffPeriod) -> TariffSnapshot {
        TariffSnapshot {
            tariff_id: self.id,
            category_id: self.category_id,
            name: self.name.clone(),
            period_id: period.id,
            days: period.days,
            devices_included: self.devices.included,
            max_extra_devices: self.devices.max_extra(),
            traffic: self.traffic,
            traffic_leftover: self.traffic_leftover(),
            squads: self.squads.clone(),
            external_squad: self.external_squad,
        }
    }

    pub fn traffic_leftover(&self) -> TrafficLeftover {
        self.traffic_addon
            .as_ref()
            .map(|a| a.leftover)
            .unwrap_or_default()
    }

    pub fn traffic_packs(&self, currency: Currency) -> Vec<&TrafficPack> {
        if !self.traffic.is_limited() {
            return Vec::new();
        }
        self.traffic_addon
            .as_ref()
            .map(|a| a.offered(currency))
            .unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.name.trim().is_empty() {
            return Err(DomainError::invalid("name", "название тарифа не задано"));
        }
        self.traffic.validate()?;
        for period in &self.periods {
            if period.days == 0 {
                return Err(DomainError::invalid("days", "срок должен быть больше нуля"));
            }
            period.prices.validate("price")?;
            if matches!(period.stars, Some(Stars(n)) if n <= 0) {
                return Err(DomainError::invalid(
                    "stars",
                    "цена в звёздах должна быть больше нуля",
                ));
            }
        }
        if let Some(extra) = &self.devices.extra {
            extra.monthly.validate("extra_device_price")?;
            check_device_discounts(&self.device_discounts, extra.max)?;
        }
        if let Some(addon) = &self.traffic_addon {
            addon.validate(self.traffic.is_limited())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TariffSnapshot {
    pub tariff_id: TariffId,
    pub category_id: CategoryId,
    pub name: String,
    pub period_id: PeriodId,
    pub days: u32,
    pub devices_included: u32,
    pub max_extra_devices: u32,
    pub traffic: TrafficPolicy,
    #[serde(default)]
    pub traffic_leftover: TrafficLeftover,
    pub squads: Vec<SquadId>,
    pub external_squad: Option<SquadId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TariffChannels {
    pub bot: bool,
    pub cabinet: bool,
    pub miniapp: bool,
}

impl Default for TariffChannels {
    fn default() -> Self {
        TariffChannels {
            bot: true,
            cabinet: true,
            miniapp: true,
        }
    }
}

impl TariffChannels {
    pub const fn allows(self, channel: ClientChannel) -> bool {
        match channel {
            ClientChannel::Bot => self.bot,
            ClientChannel::Cabinet => self.cabinet,
            ClientChannel::Miniapp => self.miniapp,
        }
    }

    pub const fn any(self) -> bool {
        self.bot || self.cabinet || self.miniapp
    }
}

pub fn months_for_days(days: u32) -> u32 {
    ((days as u64 * 12 + 182) / 365).max(1) as u32
}

pub const SHORT_PERIOD_DAYS: u32 = 28;
const MONTH_DAYS: i128 = 30;

pub fn shorter_than_month(days: u32) -> bool {
    days < SHORT_PERIOD_DAYS
}

pub fn month_price(price: Money, days: u32, months: u32) -> Option<Money> {
    if !shorter_than_month(days) {
        return price.divide_round(months).ok();
    }
    let step = price.currency.rounding_step() as i128;
    let units = div_round_half_up(price.minor as i128 * MONTH_DAYS, days.max(1) as i128 * step);
    i64::try_from(units * step)
        .ok()
        .map(|minor| Money::new(minor, price.currency))
}

fn month_stars(stars: Stars, days: u32, months: u32) -> Option<Stars> {
    if !shorter_than_month(days) {
        return stars.divide_round(months).ok();
    }
    i64::try_from(div_round_half_up(
        stars.0 as i128 * MONTH_DAYS,
        days.max(1) as i128,
    ))
    .ok()
    .map(Stars)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeriodOffer {
    pub period_id: PeriodId,
    pub days: u32,
    pub months: u32,
    pub price: Money,
    pub per_month: Money,
    pub savings_percent: Option<u8>,
    pub stars: Option<Stars>,
    pub stars_per_month: Option<Stars>,
    pub stars_savings_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion: Option<OfferPromotion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferPromotion {
    pub id: i64,
    pub name: String,
    pub percent: u8,
    pub ends_at: DateTime<Utc>,
    pub full_price: Money,
    pub full_stars: Option<Stars>,
    #[serde(default)]
    pub until: String,
}

fn savings(price: i64, monthly_base: i64, months: u32) -> Option<u8> {
    let full = monthly_base.checked_mul(months as i64)?;
    if months <= 1 || full <= 0 || price >= full {
        return None;
    }
    let percent = div_round_half_up(100 * (full - price) as i128, full as i128);
    u8::try_from(percent).ok().filter(|p| *p > 0)
}

pub fn period_offers(
    tariff: &Tariff,
    currency: Currency,
    promotion: Option<&PromotionTerms>,
) -> Vec<PeriodOffer> {
    let periods = tariff.offered_periods(currency);
    let base = periods.iter().find(|p| p.months() == 1).copied();
    periods
        .into_iter()
        .filter_map(|p| {
            let full = p.price(currency)?;
            let months = p.months();
            let savings_percent = base
                .and_then(|b| b.price(currency))
                .and_then(|b| savings(full.minor, b.minor, months));
            let stars_savings_percent = match (p.stars, base.and_then(|b| b.stars)) {
                (Some(s), Some(b)) => savings(s.0, b.0, months),
                _ => None,
            };
            let (price, stars) = match promotion {
                Some(promotion) => (
                    discounted(full, promotion.percent),
                    p.stars.map(|s| discounted_stars(s, promotion.percent)),
                ),
                None => (full, p.stars),
            };
            let promotion =
                promotion
                    .filter(|_| price != full || stars != p.stars)
                    .map(|promotion| OfferPromotion {
                        id: promotion.id,
                        name: promotion.name.clone(),
                        percent: promotion.percent,
                        ends_at: promotion.ends_at,
                        full_price: full,
                        full_stars: p.stars,
                        until: String::new(),
                    });
            let per_month = month_price(price, p.days, months)?;
            let stars_per_month = stars.and_then(|s| month_stars(s, p.days, months));
            Some(PeriodOffer {
                period_id: p.id,
                days: p.days,
                months,
                price,
                per_month,
                savings_percent,
                stars,
                stars_per_month,
                stars_savings_percent,
                promotion,
            })
        })
        .collect()
}

pub const DESCRIPTION_MAX: usize = 2_000;
pub const DESCRIPTION_MARKUP_MAX: usize = 8_000;

pub fn clean_description(raw: &str) -> Result<Option<String>, DomainError> {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let html = normalize_static_markup(text)
        .map_err(|problem| DomainError::invalid("description", problem.message()))?;
    let length = visible_len(&html);
    if length > DESCRIPTION_MAX {
        return Err(DomainError::invalid(
            "description",
            format!("описание длиннее {DESCRIPTION_MAX} знаков, сейчас {length}"),
        ));
    }
    if html.chars().count() > DESCRIPTION_MARKUP_MAX {
        return Err(DomainError::invalid(
            "description",
            format!(
                "слишком много оформления: вместе с разметкой описание длиннее {DESCRIPTION_MARKUP_MAX} знаков"
            ),
        ));
    }
    Ok(Some(html))
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::traffic::TrafficReset;
    use uuid::Uuid;

    pub fn period(id: i64, days: u32, usd: i64, rub: i64, stars: i64) -> TariffPeriod {
        TariffPeriod {
            id: PeriodId(id),
            days,
            prices: CurrencyPrices {
                usd: Some(usd),
                rub: Some(rub * 100),
            },
            stars: Some(Stars(stars)),
            sort: 0,
            active: true,
        }
    }

    pub fn extra() -> Option<ExtraDevices> {
        Some(ExtraDevices {
            max: 2,
            monthly: CurrencyPrices {
                usd: Some(99),
                rub: Some(4900),
            },
            monthly_stars: Some(Stars(50)),
        })
    }

    pub fn standard() -> Tariff {
        Tariff {
            id: TariffId(1),
            category_id: CategoryId(1),
            name: "Стандарт".into(),
            description: None,
            on_sale: true,
            sort: 0,
            periods: vec![
                period(11, 30, 299, 199, 150),
                period(12, 90, 699, 499, 350),
                period(13, 365, 2399, 1690, 1200),
            ],
            devices: DevicePolicy {
                included: 3,
                extra: extra(),
            },
            traffic: TrafficPolicy::UNLIMITED,
            traffic_addon: None,
            squads: vec![SquadId(Uuid::from_u128(1)), SquadId(Uuid::from_u128(2))],
            external_squad: None,
            device_discounts: Vec::new(),
        }
    }

    pub fn unblock() -> Tariff {
        Tariff {
            id: TariffId(3),
            category_id: CategoryId(2),
            name: "Unblock".into(),
            description: None,
            on_sale: true,
            sort: 0,
            periods: vec![
                period(31, 30, 349, 249, 175),
                period(32, 90, 899, 649, 450),
                period(33, 365, 2999, 2190, 1500),
            ],
            devices: DevicePolicy {
                included: 3,
                extra: extra(),
            },
            traffic: TrafficPolicy {
                limit_bytes: Some(300 * GIB),
                reset: TrafficReset::Month,
            },
            traffic_addon: Some(TrafficAddon {
                packs: vec![TrafficPack {
                    gb: 50,
                    prices: CurrencyPrices {
                        usd: Some(199),
                        rub: Some(15_000),
                    },
                    stars: Some(Stars(100)),
                }],
                leftover: TrafficLeftover::Burn,
            }),
            squads: vec![SquadId(Uuid::from_u128(3)), SquadId(Uuid::from_u128(1))],
            external_squad: None,
            device_discounts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn описание_тарифа_проверяется_и_нормализуется() {
        let reason = |raw: &str| match clean_description(raw) {
            Err(DomainError::InvalidValue { reason, .. }) => reason,
            other => panic!("{raw}: {other:?}"),
        };
        assert!(matches!(clean_description("  \r\n "), Ok(None)));
        assert_eq!(
            clean_description(" Для мобильного интернета ")
                .unwrap()
                .as_deref(),
            Some("Для мобильного интернета")
        );
        assert_eq!(
            clean_description("<B>Тариф:</B> 30 дней\r\n\r\n<s>от 419 ₽</s> & скидка\n")
                .unwrap()
                .as_deref(),
            Some("<b>Тариф:</b> 30 дней\n\n<s>от 419 ₽</s> &amp; скидка")
        );
        assert_eq!(reason("<b>не закрыт"), "тег <b> не закрыт");
        assert_eq!(reason("<i>a</b>"), "лишний закрывающий тег </b>");
        assert!(reason("<a href=\"{link}\">x</a>").contains("адрес"));
        assert_eq!(
            reason(&"я".repeat(DESCRIPTION_MAX + 1)),
            "описание длиннее 2000 знаков, сейчас 2001"
        );
        assert!(clean_description(&format!("<b>{}</b>", "я".repeat(DESCRIPTION_MAX))).is_ok());
        assert!(reason(&"<b>я</b>".repeat(1_200)).contains("8000"));
    }

    #[test]
    fn месяцы_по_сроку_в_днях() {
        assert_eq!(months_for_days(30), 1);
        assert_eq!(months_for_days(90), 3);
        assert_eq!(months_for_days(180), 6);
        assert_eq!(months_for_days(365), 12);
        assert_eq!(months_for_days(7), 1);
    }

    #[test]
    fn выгода_и_цена_за_месяц_как_в_currency_md() {
        let offers = period_offers(&standard(), Currency::Usd, None);
        assert_eq!(offers.len(), 3);
        assert_eq!(offers[1].per_month, Money::new(233, Currency::Usd));
        assert_eq!(offers[1].savings_percent, Some(22));
        assert_eq!(offers[2].savings_percent, Some(33));
        assert_eq!(offers[0].savings_percent, None);
        let rub = period_offers(&standard(), Currency::Rub, None);
        assert_eq!(rub[1].savings_percent, Some(16));
        assert_eq!(rub[2].savings_percent, Some(29));
        assert_eq!(rub[2].per_month, Money::new(14_100, Currency::Rub));
        let unblock = period_offers(&unblock(), Currency::Usd, None);
        assert_eq!(unblock[1].savings_percent, Some(14));
        assert_eq!(unblock[2].savings_percent, Some(28));
        assert_eq!(unblock[1].stars_savings_percent, Some(14));
        assert_eq!(unblock[2].stars_savings_percent, Some(29));
        assert_eq!(offers[1].stars_per_month, Some(Stars(117)));
    }

    #[test]
    fn цена_в_месяц_у_срока_короче_месяца_считается_на_тридцать_дней() {
        let mut tariff = standard();
        tariff.periods = vec![period(21, 7, 100, 225, 50), period(22, 30, 300, 540, 150)];
        let rub = period_offers(&tariff, Currency::Rub, None);
        assert_eq!(rub[0].days, 7);
        assert_eq!(rub[0].price, Money::new(22_500, Currency::Rub));
        assert_eq!(rub[0].per_month, Money::new(96_400, Currency::Rub));
        assert_eq!(rub[0].stars_per_month, Some(Stars(214)));
        assert_eq!(rub[0].savings_percent, None);
        assert_eq!(rub[1].per_month, Money::new(54_000, Currency::Rub));
        let usd = period_offers(&tariff, Currency::Usd, None);
        assert_eq!(usd[0].per_month, Money::new(429, Currency::Usd));
        assert!(shorter_than_month(7));
        assert!(!shorter_than_month(28));
        assert!(!shorter_than_month(30));
    }

    #[test]
    fn срок_без_цены_в_валюте_не_продаётся() {
        let mut tariff = standard();
        tariff.periods[0].prices.rub = None;
        assert_eq!(
            tariff.sellable_period(PeriodId(11), Currency::Rub),
            Err(DomainError::NoPriceInCurrency("RUB"))
        );
        tariff.on_sale = false;
        assert_eq!(
            tariff.sellable_period(PeriodId(12), Currency::Usd),
            Err(DomainError::TariffNotOnSale(TariffId(1)))
        );
    }

    #[test]
    fn доп_устройства_в_пределах_лимита() {
        let devices = standard().devices;
        assert!(devices.check_extra(2, true).is_ok());
        assert_eq!(
            devices.check_extra(3, true),
            Err(DomainError::TooManyExtraDevices { max: 2 })
        );
        assert_eq!(
            devices.check_extra(1, false),
            Err(DomainError::ExtraDevicesNotSold)
        );
        assert_eq!(devices.max_total(), 5);
    }
    #[test]
    fn скидка_за_устройства_берётся_по_самой_большой_подходящей_ступени() {
        let mut tariff = standard();
        tariff.devices.extra.as_mut().unwrap().max = 6;
        tariff.device_discounts = vec![
            DeviceDiscount {
                from: 5,
                percent: 20,
            },
            DeviceDiscount {
                from: 2,
                percent: 10,
            },
        ];
        assert_eq!(tariff.device_discount(0), None);
        assert_eq!(tariff.device_discount(1), None);
        assert_eq!(
            tariff.device_discount(2),
            Some(DeviceDiscount {
                from: 2,
                percent: 10
            })
        );
        assert_eq!(tariff.device_discount(4).map(|d| d.percent), Some(10));
        assert_eq!(tariff.device_discount(6).map(|d| d.percent), Some(20));
        assert!(tariff.validate().is_ok());
        tariff.devices.extra = None;
        assert_eq!(tariff.device_discount(6), None);
    }

    #[test]
    fn лесенка_скидок_проверяется_до_записи() {
        let step = |from, percent| DeviceDiscount { from, percent };
        let sorted = check_device_discounts(&[step(5, 20), step(2, 10)], 6).unwrap();
        assert_eq!(sorted, vec![step(2, 10), step(5, 20)]);
        assert!(check_device_discounts(&[], 0).unwrap().is_empty());
        assert!(check_device_discounts(&[step(0, 10)], 6).is_err());
        assert!(check_device_discounts(&[step(7, 10)], 6).is_err());
        assert!(check_device_discounts(&[step(2, 0)], 6).is_err());
        assert!(check_device_discounts(&[step(2, 100)], 6).is_err());
        assert!(check_device_discounts(&[step(2, 10), step(2, 20)], 6).is_err());
        assert!(check_device_discounts(&[step(2, 20), step(4, 10)], 6).is_err());
        assert!(check_device_discounts(&[step(2, 10), step(4, 10)], 6).is_err());
        assert!(check_device_discounts(&[step(1, 1); 11], 100).is_err());
        let mut tariff = standard();
        tariff.device_discounts = vec![step(3, 10)];
        assert!(tariff.validate().is_err());
    }

    #[test]
    fn лимит_подписок_категории_действует_только_в_режиме_нескольких() {
        assert!(!subscriptions_full(false, Some(1), 3));
        assert!(!subscriptions_full(true, None, 3));
        assert!(!subscriptions_full(true, Some(2), 1));
        assert!(subscriptions_full(true, Some(2), 2));
        assert!(subscriptions_full(true, Some(1), 3));
        assert!(check_subscription_limit(Some(0)).is_err());
        assert!(check_subscription_limit(Some(101)).is_err());
        assert!(check_subscription_limit(Some(1)).is_ok());
        assert!(check_subscription_limit(None).is_ok());
    }

    #[test]
    fn пакеты_трафика_только_при_лимите() {
        let mut tariff = unblock();
        assert!(tariff.validate().is_ok());
        assert_eq!(tariff.traffic_packs(Currency::Rub).len(), 1);
        assert_eq!(tariff.traffic_packs(Currency::Rub)[0].bytes(), 50 * GIB);
        tariff.traffic = TrafficPolicy::UNLIMITED;
        assert!(tariff.validate().is_err());
        assert!(tariff.traffic_packs(Currency::Rub).is_empty());
        let mut twins = unblock();
        let pack = twins.traffic_addon.as_ref().unwrap().packs[0].clone();
        twins.traffic_addon.as_mut().unwrap().packs.push(pack);
        assert!(twins.validate().is_err());
        let mut empty = standard();
        empty.traffic_addon = Some(TrafficAddon::default());
        assert!(empty.validate().is_ok());
        assert_eq!(unblock().traffic_leftover(), TrafficLeftover::Burn);
    }
}
