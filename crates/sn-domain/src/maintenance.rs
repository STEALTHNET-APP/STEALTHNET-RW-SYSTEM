use crate::client::Locale;
use crate::format::{MONTHS_EN, MONTHS_RU, NBSP, utc_label};
use chrono::{DateTime, Datelike, FixedOffset, Offset, Timelike, Utc};
use serde::{Deserialize, Serialize};

pub const MESSAGE_MAX: usize = 500;
pub const NOTICE_EVERY_MINUTES: i32 = 10;
pub const OFFSET_MAX_MINUTES: i32 = 14 * 60;

const STANDARD_RU: &str = "Обновляем сервис. Скоро всё снова заработает, загляните чуть позже.";
const STANDARD_EN: &str =
    "We are updating the service. Everything will be back soon, please check again a little later.";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Maintenance {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub message_ru: Option<String>,
    #[serde(default)]
    pub message_en: Option<String>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub offset_minutes: i32,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Texts {
    pub ru: String,
    pub en: String,
}

pub const fn standard(locale: Locale) -> &'static str {
    match locale {
        Locale::Ru => STANDARD_RU,
        Locale::En => STANDARD_EN,
    }
}

pub fn standard_texts() -> Texts {
    Texts {
        ru: STANDARD_RU.into(),
        en: STANDARD_EN.into(),
    }
}

pub fn clean_message(text: Option<&str>) -> Option<String> {
    let text = text?.replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let joined = lines.join("\n");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn too_long(text: &str) -> bool {
    text.chars().count() > MESSAGE_MAX
}

impl Maintenance {
    pub fn own(&self, locale: Locale) -> Option<&str> {
        let text = match locale {
            Locale::Ru => self.message_ru.as_deref(),
            Locale::En => self.message_en.as_deref(),
        };
        text.map(str::trim).filter(|t| !t.is_empty())
    }

    pub fn message(&self, locale: Locale) -> String {
        self.own(locale).unwrap_or(standard(locale)).to_string()
    }

    pub fn messages(&self) -> Texts {
        Texts {
            ru: self.message(Locale::Ru),
            en: self.message(Locale::En),
        }
    }

    pub fn offset(&self) -> FixedOffset {
        let seconds = self
            .offset_minutes
            .clamp(-OFFSET_MAX_MINUTES, OFFSET_MAX_MINUTES)
            * 60;
        FixedOffset::east_opt(seconds).unwrap_or_else(|| Utc.fix())
    }

    pub fn ahead(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.until.filter(|until| *until > now)
    }

    pub fn until_line(&self, locale: Locale, now: DateTime<Utc>) -> Option<String> {
        let until = self.ahead(now)?;
        let offset = self.offset();
        let local = until.with_timezone(&offset);
        let year = (local.year() != now.with_timezone(&offset).year()).then_some(local.year());
        let time = format!("{:02}:{:02}", local.hour(), local.minute());
        let zone = utc_label(offset);
        let month = local.month0() as usize;
        Some(match locale {
            Locale::Ru => {
                let date = match year {
                    Some(year) => format!("{}{NBSP}{} {year}", local.day(), MONTHS_RU[month]),
                    None => format!("{}{NBSP}{}", local.day(), MONTHS_RU[month]),
                };
                format!("Закончим примерно {date} в{NBSP}{time} ({zone}).")
            }
            Locale::En => {
                let date = match year {
                    Some(year) => format!("{}{NBSP}{} {year}", local.day(), MONTHS_EN[month]),
                    None => format!("{}{NBSP}{}", local.day(), MONTHS_EN[month]),
                };
                format!("We expect to finish around {time} on{NBSP}{date} ({zone}).")
            }
        })
    }

    pub fn until_label(&self) -> Option<String> {
        let offset = self.offset();
        let local = self.until?.with_timezone(&offset);
        Some(format!(
            "{:02}.{:02}.{} {:02}:{:02} ({})",
            local.day(),
            local.month(),
            local.year(),
            local.hour(),
            local.minute(),
            utc_label(offset)
        ))
    }

    pub fn text(&self, locale: Locale, now: DateTime<Utc>) -> String {
        let message = self.message(locale);
        match self.until_line(locale, now) {
            Some(line) => format!("{message}\n\n{line}"),
            None => message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 26, h, m, 0).unwrap()
    }

    #[test]
    fn пустое_сообщение_заменяется_стандартным() {
        let mode = Maintenance {
            enabled: true,
            message_ru: Some("   ".into()),
            message_en: None,
            ..Maintenance::default()
        };
        assert_eq!(mode.message(Locale::Ru), STANDARD_RU);
        assert_eq!(mode.message(Locale::En), STANDARD_EN);
        let own = Maintenance {
            message_ru: Some("Обновляем серверы".into()),
            ..mode
        };
        assert_eq!(own.message(Locale::Ru), "Обновляем серверы");
        assert_eq!(own.message(Locale::En), STANDARD_EN);
    }

    #[test]
    fn время_окончания_пишется_в_поясе_оператора() {
        let mode = Maintenance {
            enabled: true,
            until: Some(at(12, 0)),
            offset_minutes: 180,
            ..Maintenance::default()
        };
        assert_eq!(
            mode.until_line(Locale::Ru, at(9, 0)).as_deref(),
            Some("Закончим примерно 26\u{a0}ноября в\u{a0}15:00 (UTC+3).")
        );
        assert_eq!(
            mode.until_line(Locale::En, at(9, 0)).as_deref(),
            Some("We expect to finish around 15:00 on\u{a0}26\u{a0}November (UTC+3).")
        );
        assert_eq!(
            mode.text(Locale::Ru, at(9, 0)),
            format!("{STANDARD_RU}\n\nЗакончим примерно 26\u{a0}ноября в\u{a0}15:00 (UTC+3).")
        );
    }

    #[test]
    fn время_окончания_для_журнала() {
        let mode = Maintenance {
            until: Some(at(12, 5)),
            offset_minutes: 180,
            ..Maintenance::default()
        };
        assert_eq!(
            mode.until_label().as_deref(),
            Some("26.11.2026 15:05 (UTC+3)")
        );
        assert_eq!(Maintenance::default().until_label(), None);
    }

    #[test]
    fn прошедшее_время_окончания_не_показывается() {
        let mode = Maintenance {
            enabled: true,
            until: Some(at(12, 0)),
            ..Maintenance::default()
        };
        assert_eq!(mode.until_line(Locale::Ru, at(12, 0)), None);
        assert_eq!(mode.text(Locale::En, at(13, 0)), STANDARD_EN);
    }

    #[test]
    fn год_пишется_только_если_он_другой() {
        let mode = Maintenance {
            enabled: true,
            until: Some(Utc.with_ymd_and_hms(2027, 1, 2, 7, 30, 0).unwrap()),
            offset_minutes: -300,
            ..Maintenance::default()
        };
        assert_eq!(
            mode.until_line(Locale::Ru, at(9, 0)).as_deref(),
            Some("Закончим примерно 2\u{a0}января 2027 в\u{a0}02:30 (UTC−5).")
        );
    }

    #[test]
    fn сообщение_чистится_от_пустых_строк_по_краям() {
        assert_eq!(
            clean_message(Some("  \n Текст \r\n вторая  \n\n")),
            Some("Текст\n вторая".into())
        );
        assert_eq!(clean_message(Some(" \n ")), None);
        assert_eq!(clean_message(None), None);
        assert!(too_long(&"я".repeat(MESSAGE_MAX + 1)));
        assert!(!too_long(&"я".repeat(MESSAGE_MAX)));
    }

    #[test]
    fn старая_установка_без_настройки_не_в_режиме_работ() {
        let mode: Maintenance = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!mode.enabled);
        assert_eq!(mode, Maintenance::default());
    }
}
