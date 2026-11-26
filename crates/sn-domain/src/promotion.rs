use crate::DomainError;
use crate::client::Locale;
use crate::format::{MONTHS_EN, MONTHS_RU, NBSP};
use crate::ids::TariffId;
use crate::money::{Money, Stars};
use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike, Utc};
use serde::{Deserialize, Serialize};

pub const PERCENT_MIN: u8 = 1;
pub const PERCENT_MAX: u8 = 90;
pub const NAME_MAX: usize = 40;
pub const TARIFFS_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Promotion {
    pub id: i64,
    pub name: String,
    pub percent: u8,
    pub tariff_ids: Vec<TariffId>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionDraft {
    pub name: String,
    pub percent: u8,
    pub tariff_ids: Vec<TariffId>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionState {
    Off,
    Scheduled,
    Running,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionTerms {
    pub id: i64,
    pub name: String,
    pub percent: u8,
    pub ends_at: DateTime<Utc>,
}

impl PromotionDraft {
    pub fn checked(mut self, now: DateTime<Utc>) -> Result<PromotionDraft, DomainError> {
        self.name = self.name.split_whitespace().collect::<Vec<_>>().join(" ");
        if self.name.is_empty() {
            return Err(DomainError::invalid("name", "название акции не задано"));
        }
        if self.name.chars().count() > NAME_MAX {
            return Err(DomainError::invalid(
                "name",
                format!("название акции — не длиннее {NAME_MAX} знаков"),
            ));
        }
        if !(PERCENT_MIN..=PERCENT_MAX).contains(&self.percent) {
            return Err(DomainError::invalid(
                "percent",
                format!("скидка — от {PERCENT_MIN} до {PERCENT_MAX} %"),
            ));
        }
        let mut ids = Vec::with_capacity(self.tariff_ids.len());
        for id in &self.tariff_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        if ids.is_empty() {
            return Err(DomainError::invalid(
                "tariff_ids",
                "выберите хотя бы один тариф",
            ));
        }
        if ids.len() > TARIFFS_MAX {
            return Err(DomainError::invalid(
                "tariff_ids",
                "слишком много тарифов в одной акции",
            ));
        }
        self.tariff_ids = ids;
        if self.ends_at <= self.starts_at {
            return Err(DomainError::invalid(
                "ends_at",
                "конец акции должен быть позже начала",
            ));
        }
        if self.ends_at <= now {
            return Err(DomainError::invalid("ends_at", "конец акции уже прошёл"));
        }
        Ok(self)
    }
}

impl Promotion {
    pub fn runs_at(&self, now: DateTime<Utc>) -> bool {
        self.enabled && self.starts_at <= now && now < self.ends_at
    }

    pub fn covers(&self, tariff_id: TariffId) -> bool {
        self.tariff_ids.contains(&tariff_id)
    }

    pub fn state(&self, now: DateTime<Utc>) -> PromotionState {
        if now >= self.ends_at {
            PromotionState::Finished
        } else if !self.enabled {
            PromotionState::Off
        } else if now < self.starts_at {
            PromotionState::Scheduled
        } else {
            PromotionState::Running
        }
    }

    pub fn terms(&self) -> PromotionTerms {
        PromotionTerms {
            id: self.id,
            name: self.name.clone(),
            percent: self.percent,
            ends_at: self.ends_at,
        }
    }
}

pub fn running_for(
    promotions: &[Promotion],
    tariff_id: TariffId,
    now: DateTime<Utc>,
) -> Option<PromotionTerms> {
    promotions
        .iter()
        .filter(|p| p.runs_at(now) && p.covers(tariff_id))
        .min_by_key(|p| (std::cmp::Reverse(p.percent), p.ends_at, p.id))
        .map(Promotion::terms)
}

pub fn discounted(price: Money, percent: u8) -> Money {
    if price.minor <= 0 || percent == 0 {
        return price;
    }
    let percent = percent.min(PERCENT_MAX);
    let step = i128::from(price.currency.rounding_step());
    let kept = i128::from(price.minor) * i128::from(100 - percent) / (100 * step) * step;
    match i64::try_from(kept) {
        Ok(0) | Err(_) => price,
        Ok(minor) => Money::new(minor.min(price.minor), price.currency),
    }
}

pub fn discounted_stars(stars: Stars, percent: u8) -> Stars {
    if stars.0 <= 0 || percent == 0 {
        return stars;
    }
    let percent = percent.min(PERCENT_MAX);
    let kept = (i128::from(stars.0) * i128::from(100 - percent) + 99) / 100;
    match i64::try_from(kept) {
        Ok(0) | Err(_) => stars,
        Ok(value) => Stars(value.min(stars.0)),
    }
}

pub fn line_label(name: &str, percent: u8) -> String {
    format!("{name}, −{percent}%")
}

pub fn until_text(
    ends_at: DateTime<Utc>,
    offset: FixedOffset,
    locale: Locale,
    now: DateTime<Utc>,
) -> String {
    let end = ends_at.with_timezone(&offset);
    let midnight = end.hour() == 0 && end.minute() == 0;
    let whole_day = midnight || (end.hour() == 23 && end.minute() == 59);
    let day = if midnight {
        (ends_at - Duration::seconds(1)).with_timezone(&offset)
    } else {
        end
    };
    let index = day.month0() as usize;
    let year = (day.year() != now.with_timezone(&offset).year()).then_some(day.year());
    let time = (!whole_day).then(|| format!("{:02}:{:02}", end.hour(), end.minute()));
    match locale {
        Locale::Ru => {
            let mut text = format!("до {}{NBSP}{}", day.day(), MONTHS_RU[index]);
            if let Some(year) = year {
                text.push_str(&format!(" {year}"));
            }
            if let Some(time) = time {
                text.push_str(&format!(", {time}"));
            }
            text
        }
        Locale::En => {
            let mut text = format!("until {}{NBSP}{}", MONTHS_EN[index], day.day());
            if let Some(year) = year {
                text.push_str(&format!(", {year}"));
            }
            if let Some(time) = time {
                text.push_str(&format!(", {time}"));
            }
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use chrono::TimeZone;

    fn rub(rubles: i64) -> Money {
        Money::new(rubles * 100, Currency::Rub)
    }

    fn usd(cents: i64) -> Money {
        Money::new(cents, Currency::Usd)
    }

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .unwrap()
    }

    fn promotion(id: i64, percent: u8, tariffs: &[i64]) -> Promotion {
        Promotion {
            id,
            name: "Чёрная пятница".into(),
            percent,
            tariff_ids: tariffs.iter().map(|t| TariffId(*t)).collect(),
            starts_at: at(27, 0, 0),
            ends_at: at(30, 21, 0),
            enabled: true,
        }
    }

    #[test]
    fn цена_со_скидкой_вниз_до_рубля_и_цента() {
        assert_eq!(discounted(rub(199), 25), Money::new(14_900, Currency::Rub));
        assert_eq!(discounted(rub(299), 25), rub(224));
        assert_eq!(discounted(rub(1690), 25), rub(1267));
        assert_eq!(discounted(rub(100), 25), rub(75));
        assert_eq!(discounted(Money::new(19_950, Currency::Rub), 25), rub(149));
        assert_eq!(discounted(usd(299), 25), usd(224));
        assert_eq!(discounted(usd(2399), 25), usd(1799));
        assert_eq!(discounted(usd(699), 33), usd(468));
        assert_eq!(discounted(rub(499), 90), rub(49));
        assert_eq!(discounted(rub(499), 1), rub(494));
    }

    #[test]
    fn скидка_не_делает_цену_нулевой_и_не_трогает_бесплатный_срок() {
        assert_eq!(discounted(rub(1), 90), rub(1));
        assert_eq!(
            discounted(Money::new(50, Currency::Rub), 50),
            Money::new(50, Currency::Rub)
        );
        assert_eq!(discounted(usd(1), 50), usd(1));
        assert_eq!(discounted(rub(0), 25), rub(0));
        assert_eq!(discounted(rub(299), 0), rub(299));
        assert_eq!(discounted(rub(299), 200), rub(29));
    }

    #[test]
    fn звёзды_со_скидкой_вверх_до_целой() {
        assert_eq!(discounted_stars(Stars(150), 25), Stars(113));
        assert_eq!(discounted_stars(Stars(100), 25), Stars(75));
        assert_eq!(discounted_stars(Stars(1), 90), Stars(1));
        assert_eq!(discounted_stars(Stars(1200), 33), Stars(804));
        assert_eq!(discounted_stars(Stars(7), 50), Stars(4));
        assert_eq!(discounted_stars(Stars(0), 50), Stars(0));
    }

    #[test]
    fn акция_действует_с_начала_и_до_конца() {
        let p = promotion(1, 25, &[1]);
        assert!(!p.runs_at(at(26, 23, 59)));
        assert!(p.runs_at(at(27, 0, 0)));
        assert!(p.runs_at(at(30, 20, 59)));
        assert!(!p.runs_at(at(30, 21, 0)));
        assert_eq!(p.state(at(26, 12, 0)), PromotionState::Scheduled);
        assert_eq!(p.state(at(28, 12, 0)), PromotionState::Running);
        assert_eq!(p.state(at(30, 21, 0)), PromotionState::Finished);
        let off = Promotion {
            enabled: false,
            ..p.clone()
        };
        assert!(!off.runs_at(at(28, 12, 0)));
        assert_eq!(off.state(at(28, 12, 0)), PromotionState::Off);
        assert_eq!(off.state(at(30, 22, 0)), PromotionState::Finished);
    }

    #[test]
    fn из_двух_акций_на_тариф_берётся_большая() {
        let list = [
            promotion(1, 10, &[1, 2]),
            promotion(2, 25, &[2]),
            Promotion {
                enabled: false,
                ..promotion(3, 50, &[1, 2])
            },
        ];
        let now = at(28, 12, 0);
        assert_eq!(running_for(&list, TariffId(1), now).unwrap().id, 1);
        assert_eq!(running_for(&list, TariffId(2), now).unwrap().percent, 25);
        assert!(running_for(&list, TariffId(3), now).is_none());
        assert!(running_for(&list, TariffId(1), at(30, 21, 0)).is_none());
    }

    #[test]
    fn проверка_полей_акции() {
        let now = at(26, 12, 0);
        let draft = PromotionDraft {
            name: "  Чёрная   пятница ".into(),
            percent: 25,
            tariff_ids: vec![TariffId(2), TariffId(1), TariffId(2)],
            starts_at: at(27, 0, 0),
            ends_at: at(30, 21, 0),
            enabled: true,
        };
        let checked = draft.clone().checked(now).unwrap();
        assert_eq!(checked.name, "Чёрная пятница");
        assert_eq!(checked.tariff_ids, vec![TariffId(2), TariffId(1)]);
        for (broken, field) in [
            (
                PromotionDraft {
                    name: " ".into(),
                    ..draft.clone()
                },
                "name",
            ),
            (
                PromotionDraft {
                    percent: 0,
                    ..draft.clone()
                },
                "percent",
            ),
            (
                PromotionDraft {
                    percent: 91,
                    ..draft.clone()
                },
                "percent",
            ),
            (
                PromotionDraft {
                    tariff_ids: Vec::new(),
                    ..draft.clone()
                },
                "tariff_ids",
            ),
            (
                PromotionDraft {
                    ends_at: at(27, 0, 0),
                    ..draft.clone()
                },
                "ends_at",
            ),
            (
                PromotionDraft {
                    starts_at: at(20, 0, 0),
                    ends_at: at(25, 0, 0),
                    ..draft.clone()
                },
                "ends_at",
            ),
        ] {
            match broken.checked(now) {
                Err(DomainError::InvalidValue { field: got, .. }) => assert_eq!(got, field),
                other => panic!("{field}: {other:?}"),
            }
        }
    }

    #[test]
    fn подпись_до_какого_числа() {
        let msk = FixedOffset::east_opt(3 * 3600).unwrap();
        let now = at(26, 12, 0);
        let midnight = Utc.with_ymd_and_hms(2026, 11, 30, 21, 0, 0).unwrap();
        assert_eq!(
            until_text(midnight, msk, Locale::Ru, now),
            "до 30\u{a0}ноября"
        );
        assert_eq!(
            until_text(midnight, msk, Locale::En, now),
            "until November\u{a0}30"
        );
        let late = Utc.with_ymd_and_hms(2026, 11, 30, 20, 59, 0).unwrap();
        assert_eq!(until_text(late, msk, Locale::Ru, now), "до 30\u{a0}ноября");
        let evening = Utc.with_ymd_and_hms(2026, 11, 30, 15, 0, 0).unwrap();
        assert_eq!(
            until_text(evening, msk, Locale::Ru, now),
            "до 30\u{a0}ноября, 18:00"
        );
        let next_year = Utc.with_ymd_and_hms(2027, 1, 10, 21, 0, 0).unwrap();
        assert_eq!(
            until_text(next_year, msk, Locale::Ru, now),
            "до 10\u{a0}января 2027"
        );
        assert_eq!(line_label("Чёрная пятница", 25), "Чёрная пятница, −25%");
    }
}
