use crate::DomainError;
use crate::client::Locale;
use crate::format;
use crate::ids::{PeriodId, TariffId};
use crate::module::ModuleId;
use crate::money::{Amount, Currency, Money, PayCurrency, Stars, div_round_half_up};
use crate::promotion::{PromotionTerms, discounted, discounted_stars, line_label};
use crate::tariff::Tariff;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdjustmentKind {
    Percent { percent: u8 },
    Fixed { amount: Money },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdjustmentSource {
    PersonalDiscount,
    Promo { code: String, promo_id: Option<i64> },
    Module { module: ModuleId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceAdjustment {
    pub source: AdjustmentSource,
    pub kind: AdjustmentKind,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuoteLineKind {
    Period {
        tariff_id: TariffId,
        period_id: PeriodId,
        days: u32,
        months: u32,
    },
    ExtraDevices {
        count: u32,
        months: u32,
    },
    DeviceAddon {
        count: u32,
        days: u32,
    },
    DeviceDiscount {
        count: u32,
        percent: u8,
    },
    TrafficAddon {
        gb: u32,
    },
    Adjustment {
        source: AdjustmentSource,
    },
    Promotion {
        promotion_id: i64,
        percent: u8,
    },
    TopUp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteLine {
    pub kind: QuoteLineKind,
    pub label: String,
    pub amount: Money,
    pub stars: Option<Stars>,
}

impl QuoteLine {
    pub fn shown_label(&self) -> String {
        match crate::lang::current() {
            Locale::Ru => self.label.clone(),
            Locale::En => self.english_label(),
        }
    }

    fn english_label(&self) -> String {
        let en = Locale::En;
        match &self.kind {
            QuoteLineKind::Period { days, .. } => match self.label.strip_prefix("Подарок: «")
            {
                Some(rest) => {
                    let name = rest.rsplit_once("» на ").map_or(rest, |(name, _)| name);
                    format!("Gift: “{name}” for {}", format::term_in(*days, en))
                }
                None => {
                    let name = self
                        .label
                        .rsplit_once(" · ")
                        .map_or(self.label.as_str(), |(name, _)| name);
                    format!("{name} · {}", format::days_in(*days, en))
                }
            },
            QuoteLineKind::ExtraDevices { count, months } => {
                format!(
                    "Extra devices: {count} × {}",
                    format::months_in(*months, en)
                )
            }
            QuoteLineKind::DeviceAddon { count, days } => {
                format!("Extra devices: +{count} for {}", format::days_in(*days, en))
            }
            QuoteLineKind::DeviceDiscount { count, percent } => {
                device_discount_label(*count, *percent, en)
            }
            QuoteLineKind::TrafficAddon { gb } => {
                format!("Traffic: +{}", format::gigabytes_in(u64::from(*gb), en))
            }
            QuoteLineKind::TopUp => "Balance top-up".into(),
            QuoteLineKind::Promotion { .. } => self.label.clone(),
            QuoteLineKind::Adjustment { source } => {
                let cut = match self.label.rsplit_once(", ") {
                    Some((_, tail)) if tail.ends_with('%') => tail.to_string(),
                    _ => format::money_in(self.amount, en),
                };
                match source {
                    AdjustmentSource::Promo { code, .. } => format!("Promo code {code}, {cut}"),
                    AdjustmentSource::PersonalDiscount => format!("Personal discount, {cut}"),
                    AdjustmentSource::Module { .. } => format!("Discount, {cut}"),
                }
            }
        }
    }
}

impl Quote {
    pub fn shown(mut self) -> Self {
        for line in &mut self.lines {
            line.label = line.shown_label();
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarsTotal {
    pub subtotal: Stars,
    pub discount: Stars,
    pub total: Stars,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub currency: Currency,
    pub lines: Vec<QuoteLine>,
    pub subtotal: Money,
    pub discount: Money,
    pub total: Money,
    pub stars: Option<StarsTotal>,
    pub days: u32,
    pub extra_devices: u32,
}

impl Quote {
    pub const fn is_free(&self) -> bool {
        self.total.minor == 0
    }

    pub fn amount_in(&self, currency: PayCurrency) -> Option<Amount> {
        match currency {
            PayCurrency::Xtr => self.stars.map(|s| s.total.into()),
            other if other == PayCurrency::from(self.currency) => Some(self.total.into()),
            _ => None,
        }
    }
}

pub const MONTH_DAYS: u32 = 30;

pub fn device_discount_label(count: u32, percent: u8, locale: Locale) -> String {
    let devices = format::count_in(
        u64::from(count),
        locale,
        ["доп. устройство", "доп. устройства", "доп. устройств"],
        ["extra device", "extra devices"],
    );
    match locale {
        Locale::Ru => format!("Скидка за {devices}, −{percent}%"),
        Locale::En => format!("Discount for {devices}, −{percent}%"),
    }
}

pub fn device_discount_line(
    count: u32,
    percent: u8,
    amount: Money,
    stars: Option<Stars>,
) -> Result<Option<QuoteLine>, DomainError> {
    let cut = amount.percent(percent)?.min(amount)?;
    let stars_cut = match stars {
        Some(stars) => Some(stars.percent(percent)?),
        None => None,
    };
    if cut.is_zero() && stars_cut.is_none_or(|s| s.0 == 0) {
        return Ok(None);
    }
    Ok(Some(QuoteLine {
        kind: QuoteLineKind::DeviceDiscount { count, percent },
        label: device_discount_label(count, percent, Locale::Ru),
        amount: cut.checked_neg()?,
        stars: stars_cut.map(|s| Stars(-s.0)),
    }))
}

pub fn days_left(expire_at: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    let seconds = (expire_at - now).num_seconds();
    if seconds <= 0 {
        return 0;
    }
    u32::try_from((seconds + 86_399) / 86_400).unwrap_or(u32::MAX)
}

pub fn prorated(monthly: Money, units: u32, days: u32) -> Result<Money, DomainError> {
    if monthly.is_negative() {
        return Err(DomainError::NegativeAmount);
    }
    if monthly.is_zero() || units == 0 || days == 0 {
        return Ok(Money::zero(monthly.currency));
    }
    let step = i128::from(monthly.currency.rounding_step());
    let numerator = i128::from(monthly.minor) * i128::from(units) * i128::from(days);
    let steps = div_round_half_up(numerator, i128::from(MONTH_DAYS) * step).max(1);
    let minor = i64::try_from(steps * step).map_err(|_| DomainError::AmountOverflow)?;
    Ok(Money::new(minor, monthly.currency))
}

pub fn prorated_stars(monthly: Stars, units: u32, days: u32) -> Result<Stars, DomainError> {
    if monthly.0 <= 0 || units == 0 || days == 0 {
        return Ok(Stars::ZERO);
    }
    let numerator = i128::from(monthly.0) * i128::from(units) * i128::from(days);
    let month = i128::from(MONTH_DAYS);
    let stars = ((numerator + month - 1) / month).max(1);
    Ok(Stars(
        i64::try_from(stars).map_err(|_| DomainError::AmountOverflow)?,
    ))
}

#[derive(Debug, Clone, Copy)]
pub struct QuoteRequest<'a> {
    pub currency: Currency,
    pub tariff: &'a Tariff,
    pub period_id: PeriodId,
    pub extra_devices: u32,
    pub sell_extra_devices: bool,
    pub adjustments: &'a [PriceAdjustment],
    pub promotion: Option<&'a PromotionTerms>,
}

fn add_stars(left: Option<Stars>, right: Option<Stars>) -> Result<Option<Stars>, DomainError> {
    match (left, right) {
        (Some(l), Some(r)) => l.checked_add(r).map(Some),
        _ => Ok(None),
    }
}

pub fn quote(req: &QuoteRequest<'_>) -> Result<Quote, DomainError> {
    let Some(promotion) = req.promotion else {
        return priced(req, None, req.adjustments);
    };
    if req.adjustments.is_empty() {
        return priced(req, Some(promotion), &[]);
    }
    let promoted = priced(req, Some(promotion), &[])?;
    let adjusted = priced(req, None, req.adjustments)?;
    Ok(if adjusted.total.minor < promoted.total.minor {
        adjusted
    } else {
        promoted
    })
}

pub fn promotion_line(quote: &Quote) -> Option<(i64, u8)> {
    quote.lines.iter().find_map(|line| match line.kind {
        QuoteLineKind::Promotion {
            promotion_id,
            percent,
        } => Some((promotion_id, percent)),
        _ => None,
    })
}

fn priced(
    req: &QuoteRequest<'_>,
    promotion: Option<&PromotionTerms>,
    adjustments: &[PriceAdjustment],
) -> Result<Quote, DomainError> {
    let tariff = req.tariff;
    let period = tariff.sellable_period(req.period_id, req.currency)?;
    tariff
        .devices
        .check_extra(req.extra_devices, req.sell_extra_devices)?;
    let months = period.months();
    let base = period
        .price(req.currency)
        .ok_or(DomainError::NoPriceInCurrency(req.currency.code()))?;
    let mut lines = vec![QuoteLine {
        kind: QuoteLineKind::Period {
            tariff_id: tariff.id,
            period_id: period.id,
            days: period.days,
            months,
        },
        label: format!("{} · {}", tariff.name, format::days(period.days)),
        amount: base,
        stars: period.stars,
    }];
    let mut subtotal = base;
    let mut stars_subtotal = period.stars;
    let mut device_cut = Money::zero(req.currency);
    let mut device_stars_cut: Option<Stars> = None;
    let mut promotion_cut = Money::zero(req.currency);
    let mut promotion_stars_cut: Option<Stars> = None;
    if let Some(promotion) = promotion {
        let cut = base.checked_sub(discounted(base, promotion.percent))?;
        let stars_cut = match period.stars {
            Some(full) => Some(full.checked_sub(discounted_stars(full, promotion.percent))?),
            None => None,
        };
        if !cut.is_zero() || stars_cut.is_some_and(|s| s.0 > 0) {
            lines.push(QuoteLine {
                kind: QuoteLineKind::Promotion {
                    promotion_id: promotion.id,
                    percent: promotion.percent,
                },
                label: line_label(&promotion.name, promotion.percent),
                amount: cut.checked_neg()?,
                stars: stars_cut.map(|s| Stars(-s.0)),
            });
            promotion_cut = cut;
            promotion_stars_cut = stars_cut;
        }
    }

    if req.extra_devices > 0 {
        let extra = tariff
            .devices
            .extra
            .as_ref()
            .ok_or(DomainError::ExtraDevicesNotSold)?;
        let monthly = extra
            .monthly
            .get(req.currency)
            .ok_or(DomainError::NoPriceInCurrency(req.currency.code()))?;
        let units = req.extra_devices as i64 * months as i64;
        let amount = monthly.checked_mul(units)?;
        let stars = match extra.monthly_stars {
            Some(s) => Some(s.checked_mul(units)?),
            None => None,
        };
        subtotal = subtotal.checked_add(amount)?;
        stars_subtotal = add_stars(stars_subtotal, stars)?;
        lines.push(QuoteLine {
            kind: QuoteLineKind::ExtraDevices {
                count: req.extra_devices,
                months,
            },
            label: format!(
                "Доп. устройства: {} × {months}{}{}",
                req.extra_devices,
                format::NBSP,
                format::plural_ru(months as u64, "месяц", "месяца", "месяцев")
            ),
            amount,
            stars,
        });
        if let Some(step) = tariff.device_discount(req.extra_devices)
            && let Some(line) =
                device_discount_line(req.extra_devices, step.percent, amount, stars)?
        {
            device_cut = line.amount.checked_neg()?;
            device_stars_cut = line.stars.map(|s| Stars(-s.0));
            lines.push(line);
        }
    }

    let mut total = subtotal
        .checked_sub(device_cut)?
        .checked_sub(promotion_cut)?;
    let mut stars_total = stars_subtotal;
    for cut in [device_stars_cut, promotion_stars_cut]
        .into_iter()
        .flatten()
    {
        if let Some(stars) = stars_total {
            stars_total = Some(stars.checked_sub(cut)?);
        }
    }
    for adjustment in adjustments {
        let (cut, stars_cut) = match &adjustment.kind {
            AdjustmentKind::Percent { percent } => {
                let cut = total.percent(*percent)?.min(total)?;
                let stars_cut = match stars_total {
                    Some(s) => Some(s.percent(*percent)?),
                    None => None,
                };
                (cut, stars_cut)
            }
            AdjustmentKind::Fixed { amount } => {
                if amount.is_negative() {
                    return Err(DomainError::NegativeAmount);
                }
                (amount.min(total)?, stars_total.map(|_| Stars::ZERO))
            }
        };
        total = total.checked_sub(cut)?;
        if let (Some(current), Some(cut)) = (stars_total, stars_cut) {
            stars_total = Some(current.checked_sub(cut)?);
        }
        lines.push(QuoteLine {
            kind: QuoteLineKind::Adjustment {
                source: adjustment.source.clone(),
            },
            label: adjustment.label.clone(),
            amount: cut.checked_neg()?,
            stars: stars_cut.map(|s| Stars(-s.0)),
        });
    }

    Ok(Quote {
        currency: req.currency,
        lines,
        subtotal,
        discount: subtotal.checked_sub(total)?,
        total,
        stars: match (stars_subtotal, stars_total) {
            (Some(subtotal), Some(total)) => Some(StarsTotal {
                subtotal,
                discount: subtotal.checked_sub(total)?,
                total,
            }),
            _ => None,
        },
        days: period.days,
        extra_devices: req.extra_devices,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpLimits {
    pub min: Money,
    pub max: Money,
}

pub fn topup_presets(limits: TopUpLimits) -> Vec<Money> {
    let currency = limits.min.currency;
    let steps: [i64; 4] = match currency {
        Currency::Usd => [500, 1_000, 2_000, 5_000],
        Currency::Rub => [30_000, 50_000, 100_000, 200_000],
    };
    steps
        .into_iter()
        .filter(|minor| (limits.min.minor..=limits.max.minor).contains(minor))
        .map(|minor| Money::new(minor, currency))
        .collect()
}

pub const TOPUP_BONUS_MAX: u8 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BonusTerms {
    pub percent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_minor: Option<i64>,
}

impl BonusTerms {
    pub fn reached(&self, amount: Money) -> bool {
        amount.minor > 0 && self.min_minor.is_none_or(|min| amount.minor >= min)
    }

    pub fn bonus(&self, amount: Money) -> Money {
        if !self.reached(amount) {
            return Money::zero(amount.currency);
        }
        let percent = self.percent.min(TOPUP_BONUS_MAX);
        let share = i128::from(amount.minor) * i128::from(percent) / 100;
        let minor = i64::try_from(share).unwrap_or(0);
        Money::new(
            self.max_minor.map_or(minor, |max| minor.min(max)).max(0),
            amount.currency,
        )
    }
}

pub fn bonus_taken(left: i64, remaining: i64, refund: i64) -> i64 {
    if left <= 0 || refund <= 0 {
        return 0;
    }
    if refund >= remaining {
        return left;
    }
    let kept = i128::from(left) * i128::from(remaining - refund) / i128::from(remaining);
    left - i64::try_from(kept).unwrap_or(left)
}

pub fn topup_quote(amount: Money, limits: TopUpLimits) -> Result<Quote, DomainError> {
    if amount.currency != limits.min.currency {
        return Err(DomainError::CurrencyMismatch {
            left: amount.currency.code(),
            right: limits.min.currency.code(),
        });
    }
    if amount.minor < limits.min.minor || amount.minor > limits.max.minor {
        return Err(DomainError::TopUpOutOfRange {
            min: format::money(limits.min),
            max: format::money(limits.max),
        });
    }
    Ok(Quote {
        currency: amount.currency,
        lines: vec![QuoteLine {
            kind: QuoteLineKind::TopUp,
            label: "Пополнение баланса".into(),
            amount,
            stars: None,
        }],
        subtotal: amount,
        discount: Money::zero(amount.currency),
        total: amount,
        stars: None,
        days: 0,
        extra_devices: 0,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BalanceCheck {
    Enough,
    Short { balance: Money, missing: Money },
}

pub fn balance_check(balance: Money, total: Money) -> Result<BalanceCheck, DomainError> {
    if balance.minor >= total.minor {
        balance.checked_sub(total)?;
        Ok(BalanceCheck::Enough)
    } else {
        Ok(BalanceCheck::Short {
            balance,
            missing: total.checked_sub(balance)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tariff::DeviceDiscount;
    use crate::tariff::fixtures::{standard, unblock};

    fn promo(percent: u8) -> PriceAdjustment {
        PriceAdjustment {
            source: AdjustmentSource::Promo {
                code: "ZIMA".into(),
                promo_id: Some(1),
            },
            kind: AdjustmentKind::Percent { percent },
            label: "Промокод ZIMA".into(),
        }
    }

    #[test]
    fn подписи_строк_по_английски_только_при_показе() {
        let tariff = standard();
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(12),
            extra_devices: 2,
            sell_extra_devices: true,
            adjustments: &[promo(10)],
            promotion: None,
        })
        .unwrap();
        let russian: Vec<String> = q.lines.iter().map(|l| l.label.clone()).collect();
        assert_eq!(
            q.lines
                .iter()
                .map(QuoteLine::shown_label)
                .collect::<Vec<_>>(),
            russian
        );
        let english = crate::lang::sync_scope(Locale::En, || q.clone().shown());
        let labels: Vec<&str> = english.lines.iter().map(|l| l.label.as_str()).collect();
        assert!(labels[0].ends_with(" · 90\u{a0}days"), "{labels:?}");
        assert_eq!(labels[1], "Extra devices: 2 × 3\u{a0}months");
        assert!(labels[2].starts_with("Promo code ZIMA, "), "{labels:?}");
        assert_eq!(q.lines[0].label, russian[0]);
    }

    #[test]
    fn стандарт_на_три_месяца_с_двумя_устройствами() {
        let tariff = standard();
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(12),
            extra_devices: 2,
            sell_extra_devices: true,
            adjustments: &[],
            promotion: None,
        })
        .unwrap();
        assert_eq!(q.subtotal, Money::new(699 + 2 * 99 * 3, Currency::Usd));
        assert_eq!(q.total, q.subtotal);
        assert_eq!(q.stars.unwrap().total, Stars(350 + 2 * 50 * 3));
        assert_eq!(q.days, 90);
    }

    #[test]
    fn промокод_минус_десять_процентов_и_в_звёздах() {
        let tariff = standard();
        let adjustments = [promo(10)];
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(12),
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &adjustments,
            promotion: None,
        })
        .unwrap();
        assert_eq!(q.total, Money::new(629, Currency::Usd));
        assert_eq!(q.discount, Money::new(70, Currency::Usd));
        assert_eq!(q.stars.unwrap().total, Stars(315));
        assert_eq!(q.stars.unwrap().discount, Stars(35));
    }

    #[test]
    fn рубли_со_скидкой_остаются_целыми() {
        let tariff = unblock();
        let adjustments = [promo(10)];
        let q = quote(&QuoteRequest {
            currency: Currency::Rub,
            tariff: &tariff,
            period_id: PeriodId(32),
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &adjustments,
            promotion: None,
        })
        .unwrap();
        assert_eq!(q.total, Money::new(58_400, Currency::Rub));
        assert_eq!(q.total.minor % 100, 0);
    }

    #[test]
    fn фиксированная_скидка_не_уводит_в_минус_и_не_трогает_звёзды() {
        let tariff = standard();
        let adjustments = [PriceAdjustment {
            source: AdjustmentSource::PersonalDiscount,
            kind: AdjustmentKind::Fixed {
                amount: Money::new(10_000, Currency::Usd),
            },
            label: "Скидка".into(),
        }];
        let q = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(11),
            extra_devices: 0,
            sell_extra_devices: true,
            adjustments: &adjustments,
            promotion: None,
        })
        .unwrap();
        assert!(q.is_free());
        assert_eq!(q.stars.unwrap().total, Stars(150));
    }

    #[test]
    fn устройства_без_модуля_не_продаются() {
        let tariff = standard();
        let err = quote(&QuoteRequest {
            currency: Currency::Usd,
            tariff: &tariff,
            period_id: PeriodId(11),
            extra_devices: 1,
            sell_extra_devices: false,
            adjustments: &[],
            promotion: None,
        })
        .unwrap_err();
        assert_eq!(err, DomainError::ExtraDevicesNotSold);
    }

    fn with_discounts(mut tariff: Tariff) -> Tariff {
        tariff.devices.extra.as_mut().unwrap().max = 6;
        tariff.device_discounts = vec![
            DeviceDiscount {
                from: 2,
                percent: 10,
            },
            DeviceDiscount {
                from: 5,
                percent: 20,
            },
        ];
        tariff
    }

    fn devices_quote(
        tariff: &Tariff,
        currency: Currency,
        period: i64,
        extra_devices: u32,
        adjustments: &[PriceAdjustment],
    ) -> Quote {
        quote(&QuoteRequest {
            currency,
            tariff,
            period_id: PeriodId(period),
            extra_devices,
            sell_extra_devices: true,
            adjustments,
            promotion: None,
        })
        .unwrap()
    }

    #[test]
    fn скидка_за_устройства_только_на_сумму_устройств() {
        let tariff = with_discounts(standard());
        let q = devices_quote(&tariff, Currency::Usd, 12, 3, &[]);
        assert_eq!(q.subtotal, Money::new(699 + 891, Currency::Usd));
        assert_eq!(
            q.lines[2].kind,
            QuoteLineKind::DeviceDiscount {
                count: 3,
                percent: 10
            }
        );
        assert_eq!(q.lines[2].amount, Money::new(-89, Currency::Usd));
        assert_eq!(q.lines[2].stars, Some(Stars(-45)));
        assert_eq!(q.discount, Money::new(89, Currency::Usd));
        assert_eq!(q.total, Money::new(699 + 891 - 89, Currency::Usd));
        assert_eq!(
            q.lines.iter().map(|l| l.amount.minor).sum::<i64>(),
            q.total.minor
        );
        let stars = q.stars.unwrap();
        assert_eq!(stars.subtotal, Stars(350 + 450));
        assert_eq!(stars.discount, Stars(45));
        assert_eq!(stars.total, Stars(755));
        let one = devices_quote(&tariff, Currency::Usd, 12, 1, &[]);
        assert_eq!(one.lines.len(), 2);
        assert_eq!(one.discount.minor, 0);
        let five = devices_quote(&tariff, Currency::Usd, 12, 5, &[]);
        assert_eq!(five.discount, Money::new(297, Currency::Usd));
        assert_eq!(five.stars.unwrap().discount, Stars(150));
        let plain = devices_quote(&standard(), Currency::Usd, 12, 2, &[]);
        assert_eq!(plain.discount.minor, 0);
    }

    #[test]
    fn промокод_считается_после_скидки_за_устройства() {
        let tariff = with_discounts(standard());
        let q = devices_quote(&tariff, Currency::Usd, 12, 3, &[promo(10)]);
        assert!(matches!(
            q.lines[2].kind,
            QuoteLineKind::DeviceDiscount { .. }
        ));
        assert!(matches!(q.lines[3].kind, QuoteLineKind::Adjustment { .. }));
        assert_eq!(q.lines[3].amount, Money::new(-150, Currency::Usd));
        assert_eq!(q.total, Money::new(1501 - 150, Currency::Usd));
        assert_eq!(q.discount, Money::new(89 + 150, Currency::Usd));
        assert_eq!(q.stars.unwrap().total, Stars(755 - 76));
        assert_eq!(
            q.lines.iter().map(|l| l.amount.minor).sum::<i64>(),
            q.total.minor
        );
    }

    #[test]
    fn скидка_за_устройства_в_рублях_до_целого_рубля() {
        let tariff = with_discounts(unblock());
        let q = devices_quote(&tariff, Currency::Rub, 32, 3, &[]);
        assert_eq!(q.lines[2].amount, Money::new(-4_400, Currency::Rub));
        assert_eq!(q.total, Money::new(64_900 + 44_100 - 4_400, Currency::Rub));
        assert_eq!(q.total.minor % 100, 0);
    }

    #[test]
    fn подпись_скидки_за_устройства_на_двух_языках() {
        let tariff = with_discounts(standard());
        let q = devices_quote(&tariff, Currency::Usd, 12, 3, &[]);
        assert_eq!(q.lines[2].label, "Скидка за 3\u{a0}доп. устройства, −10%");
        let english = crate::lang::sync_scope(Locale::En, || q.clone().shown());
        assert_eq!(
            english.lines[2].label,
            "Discount for 3\u{a0}extra devices, −10%"
        );
        assert_eq!(
            device_discount_label(1, 5, Locale::Ru),
            "Скидка за 1\u{a0}доп. устройство, −5%"
        );
        assert_eq!(
            device_discount_label(5, 20, Locale::Ru),
            "Скидка за 5\u{a0}доп. устройств, −20%"
        );
    }

    fn black_friday(percent: u8) -> PromotionTerms {
        PromotionTerms {
            id: 7,
            name: "Чёрная пятница".into(),
            percent,
            ends_at: chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 30, 21, 0, 0).unwrap(),
        }
    }

    fn promoted(
        tariff: &Tariff,
        currency: Currency,
        period: i64,
        extra_devices: u32,
        adjustments: &[PriceAdjustment],
        promotion: &PromotionTerms,
    ) -> Quote {
        quote(&QuoteRequest {
            currency,
            tariff,
            period_id: PeriodId(period),
            extra_devices,
            sell_extra_devices: true,
            adjustments,
            promotion: Some(promotion),
        })
        .unwrap()
    }

    fn lines_sum(q: &Quote) -> i64 {
        q.lines.iter().map(|l| l.amount.minor).sum()
    }

    #[test]
    fn акция_снижает_цену_срока_вниз_до_цента_и_звёзды_вверх() {
        let tariff = standard();
        let q = promoted(&tariff, Currency::Usd, 11, 0, &[], &black_friday(25));
        assert_eq!(q.lines.len(), 2);
        assert_eq!(
            q.lines[1].kind,
            QuoteLineKind::Promotion {
                promotion_id: 7,
                percent: 25
            }
        );
        assert_eq!(q.lines[1].label, "Чёрная пятница, −25%");
        assert_eq!(q.lines[1].amount, Money::new(-75, Currency::Usd));
        assert_eq!(q.lines[1].stars, Some(Stars(-37)));
        assert_eq!(q.subtotal, Money::new(299, Currency::Usd));
        assert_eq!(q.discount, Money::new(75, Currency::Usd));
        assert_eq!(q.total, Money::new(224, Currency::Usd));
        assert_eq!(lines_sum(&q), q.total.minor);
        let stars = q.stars.unwrap();
        assert_eq!(stars.total, Stars(113));
        assert_eq!(stars.discount, Stars(37));
        assert_eq!(promotion_line(&q), Some((7, 25)));
        let english = crate::lang::sync_scope(Locale::En, || q.clone().shown());
        assert_eq!(english.lines[1].label, "Чёрная пятница, −25%");
    }

    #[test]
    fn акция_в_рублях_до_целого_рубля_вниз() {
        let tariff = unblock();
        let q = promoted(&tariff, Currency::Rub, 32, 0, &[], &black_friday(25));
        assert_eq!(q.total.minor % 100, 0);
        assert_eq!(q.total, Money::new(48_600, Currency::Rub));
        let plain = promoted(&standard(), Currency::Rub, 11, 0, &[], &black_friday(25));
        assert_eq!(plain.total, Money::new(14_900, Currency::Rub));
        assert_eq!(plain.lines[1].amount, Money::new(-5_000, Currency::Rub));
    }

    #[test]
    fn доп_устройства_по_акции_без_скидки() {
        let tariff = with_discounts(standard());
        let q = promoted(&tariff, Currency::Usd, 12, 3, &[], &black_friday(25));
        assert!(matches!(q.lines[1].kind, QuoteLineKind::Promotion { .. }));
        assert_eq!(q.lines[1].amount, Money::new(-175, Currency::Usd));
        assert!(matches!(
            q.lines[2].kind,
            QuoteLineKind::ExtraDevices { .. }
        ));
        assert_eq!(q.lines[2].amount, Money::new(891, Currency::Usd));
        assert_eq!(q.lines[3].amount, Money::new(-89, Currency::Usd));
        assert_eq!(q.total, Money::new(524 + 891 - 89, Currency::Usd));
        assert_eq!(lines_sum(&q), q.total.minor);
        assert_eq!(q.stars.unwrap().total, Stars(263 + 450 - 45));
    }

    #[test]
    fn промокод_меньше_акции_не_применяется() {
        let tariff = standard();
        let q = promoted(
            &tariff,
            Currency::Usd,
            12,
            0,
            &[promo(10)],
            &black_friday(25),
        );
        assert_eq!(promotion_line(&q), Some((7, 25)));
        assert!(
            !q.lines
                .iter()
                .any(|l| matches!(l.kind, QuoteLineKind::Adjustment { .. }))
        );
        assert_eq!(q.total, Money::new(524, Currency::Usd));
        let tie = promoted(
            &tariff,
            Currency::Usd,
            12,
            0,
            &[promo(25)],
            &black_friday(25),
        );
        assert_eq!(promotion_line(&tie), Some((7, 25)));
        assert_eq!(tie.lines.len(), 2);
    }

    #[test]
    fn промокод_больше_акции_берётся_от_полной_цены() {
        let tariff = standard();
        let q = promoted(
            &tariff,
            Currency::Usd,
            12,
            0,
            &[promo(30)],
            &black_friday(25),
        );
        assert_eq!(promotion_line(&q), None);
        assert!(matches!(q.lines[1].kind, QuoteLineKind::Adjustment { .. }));
        assert_eq!(q.total, Money::new(699 - 210, Currency::Usd));
        assert_eq!(q.stars.unwrap().total, Stars(350 - 105));
        let fixed = [PriceAdjustment {
            source: AdjustmentSource::Promo {
                code: "MINUS".into(),
                promo_id: Some(2),
            },
            kind: AdjustmentKind::Fixed {
                amount: Money::new(300, Currency::Usd),
            },
            label: "Промокод MINUS".into(),
        }];
        let q = promoted(&tariff, Currency::Usd, 12, 0, &fixed, &black_friday(25));
        assert_eq!(promotion_line(&q), None);
        assert_eq!(q.total, Money::new(399, Currency::Usd));
        let devices = promoted(
            &with_discounts(standard()),
            Currency::Usd,
            12,
            2,
            &[promo(20)],
            &black_friday(25),
        );
        assert_eq!(promotion_line(&devices), None);
        assert_eq!(lines_sum(&devices), devices.total.minor);
    }

    #[test]
    fn баланс_только_на_полную_сумму() {
        let balance = Money::new(420, Currency::Usd);
        assert_eq!(
            balance_check(balance, Money::new(699, Currency::Usd)).unwrap(),
            BalanceCheck::Short {
                balance,
                missing: Money::new(279, Currency::Usd)
            }
        );
        assert_eq!(
            balance_check(balance, Money::new(420, Currency::Usd)).unwrap(),
            BalanceCheck::Enough
        );
    }

    #[test]
    fn пополнение_в_пределах() {
        let limits = TopUpLimits {
            min: Money::new(100, Currency::Usd),
            max: Money::new(100_000, Currency::Usd),
        };
        assert!(topup_quote(Money::new(1000, Currency::Usd), limits).is_ok());
        assert!(topup_quote(Money::new(50, Currency::Usd), limits).is_err());
        assert!(topup_quote(Money::new(1000, Currency::Rub), limits).is_err());
    }

    #[test]
    fn готовые_суммы_пополнения_только_в_пределах() {
        let usd = |min, max| TopUpLimits {
            min: Money::new(min, Currency::Usd),
            max: Money::new(max, Currency::Usd),
        };
        let minors = |limits| -> Vec<i64> {
            topup_presets(limits)
                .into_iter()
                .map(|money| money.minor)
                .collect()
        };
        assert_eq!(minors(usd(100, 100_000)), [500, 1_000, 2_000, 5_000]);
        assert_eq!(minors(usd(1_000, 2_000)), [1_000, 2_000]);
        assert!(minors(usd(6_000, 9_000)).is_empty());
        let rub = topup_presets(TopUpLimits {
            min: Money::new(10_000, Currency::Rub),
            max: Money::new(10_000_000, Currency::Rub),
        });
        assert_eq!(rub.len(), 4);
        assert!(rub.iter().all(|money| money.currency == Currency::Rub));
        assert_eq!(rub[0].minor, 30_000);
    }
}

#[cfg(test)]
mod proration_tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn устройство_за_полмесяца_стоит_половину() {
        let rub = |r: i64| Money::new(r * 100, Currency::Rub);
        assert_eq!(prorated(rub(50), 1, 15).unwrap(), rub(25));
        assert_eq!(prorated(rub(50), 2, 15).unwrap(), rub(50));
        assert_eq!(prorated(rub(50), 1, 30).unwrap(), rub(50));
        assert_eq!(prorated(rub(50), 1, 45).unwrap(), rub(75));
        assert_eq!(prorated(rub(50), 1, 14).unwrap(), rub(23));
        assert_eq!(prorated(rub(1), 1, 1).unwrap(), rub(1));
        let usd = |c: i64| Money::new(c, Currency::Usd);
        assert_eq!(prorated(usd(99), 1, 15).unwrap(), usd(50));
        assert_eq!(prorated(usd(99), 1, 0).unwrap(), usd(0));
        assert_eq!(prorated_stars(Stars(50), 1, 15).unwrap(), Stars(25));
        assert_eq!(prorated_stars(Stars(40), 1, 1).unwrap(), Stars(2));
        assert_eq!(prorated_stars(Stars(1), 1, 1).unwrap(), Stars(1));
    }

    #[test]
    fn оставшиеся_дни_округляются_вверх() {
        let now = Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0).unwrap();
        assert_eq!(days_left(now + Duration::days(15), now), 15);
        assert_eq!(days_left(now + Duration::hours(5), now), 1);
        assert_eq!(
            days_left(now + Duration::days(14) + Duration::hours(1), now),
            15
        );
        assert_eq!(days_left(now - Duration::hours(1), now), 0);
    }

    fn kopecks(minor: i64) -> Money {
        Money::new(minor, Currency::Rub)
    }

    const fn terms(percent: u8) -> BonusTerms {
        BonusTerms {
            percent,
            min_minor: None,
            max_minor: None,
        }
    }

    #[test]
    fn бонус_к_пополнению_округляется_вниз_до_копейки() {
        for (minor, percent, bonus) in [
            (50_000, 5, 2_500),
            (1_005, 7, 70),
            (99, 1, 0),
            (100, 1, 1),
            (199, 1, 1),
            (10_700, 5, 535),
            (333, 100, 333),
            (50_000, 0, 0),
            (0, 5, 0),
            (-500, 5, 0),
            (i64::MAX, 100, i64::MAX),
        ] {
            assert_eq!(terms(percent).bonus(kopecks(minor)), kopecks(bonus));
        }
        assert_eq!(terms(200).bonus(kopecks(1_000)), kopecks(1_000));
    }

    #[test]
    fn порог_и_предел_бонуса() {
        let limited = BonusTerms {
            percent: 10,
            min_minor: Some(50_000),
            max_minor: Some(8_000),
        };
        for (minor, bonus) in [
            (49_999, 0),
            (50_000, 5_000),
            (79_999, 7_999),
            (80_000, 8_000),
            (80_001, 8_000),
            (1_000_000, 8_000),
        ] {
            assert_eq!(limited.bonus(kopecks(minor)), kopecks(bonus), "{minor}");
        }
        assert!(!limited.reached(kopecks(49_999)));
        assert!(limited.reached(kopecks(50_000)));
        let stored = serde_json::to_value(limited).unwrap();
        assert_eq!(
            serde_json::from_value::<BonusTerms>(stored).unwrap(),
            limited
        );
        assert_eq!(
            serde_json::from_str::<BonusTerms>("{\"percent\":5}").unwrap(),
            terms(5)
        );
    }

    #[test]
    fn возвраты_по_частям_снимают_ровно_начисленный_бонус() {
        for paid in [1_i64, 7, 99, 100, 101, 1_005, 3_333] {
            for credited in [0_i64, 1, 3, 50, 70, 1_000, 3_333] {
                for part in [1_i64, 2, 13, 97, 400] {
                    let (mut remaining, mut left) = (paid, credited);
                    while remaining > 0 {
                        let refund = part.min(remaining);
                        let taken = bonus_taken(left, remaining, refund);
                        assert!((0..=left).contains(&taken));
                        left -= taken;
                        remaining -= refund;
                    }
                    assert_eq!(left, 0);
                }
            }
        }
        assert_eq!(bonus_taken(70, 1_005, 333), 24);
        assert_eq!(bonus_taken(8_000, 1_000_000, 500_000), 4_000);
        assert_eq!(bonus_taken(70, 1_005, 2_000), 70);
        assert_eq!(bonus_taken(70, 1_005, 0), 0);
        assert_eq!(bonus_taken(0, 1_005, 333), 0);
        assert_eq!(bonus_taken(i64::MAX, i64::MAX, 1), 1);
    }
}
