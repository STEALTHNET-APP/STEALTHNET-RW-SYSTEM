use crate::client::Locale;
use crate::money::{Currency, Money, Stars};
use crate::traffic::{GIB, TIB};

pub const NBSP: char = '\u{a0}';

pub const MONTHS_RU: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

pub const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub fn date_in(date: chrono::NaiveDate, locale: Locale) -> String {
    use chrono::Datelike;
    let index = date.month0() as usize;
    match locale {
        Locale::Ru => format!("{}{NBSP}{} {}", date.day(), MONTHS_RU[index], date.year()),
        Locale::En => format!("{}{NBSP}{}, {}", MONTHS_EN[index], date.day(), date.year()),
    }
}

pub fn utc_label(offset: chrono::FixedOffset) -> String {
    let minutes = offset.local_minus_utc() / 60;
    if minutes == 0 {
        return "UTC".into();
    }
    let sign = if minutes < 0 { '−' } else { '+' };
    let (h, m) = (minutes.abs() / 60, minutes.abs() % 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 * 2);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(NBSP);
        }
        out.push(ch);
    }
    out
}

pub fn plural_ru<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let last = n % 10;
    let last_two = n % 100;
    if last == 1 && last_two != 11 {
        one
    } else if (2..=4).contains(&last) && !(12..=14).contains(&last_two) {
        few
    } else {
        many
    }
}

pub fn money(value: Money) -> String {
    let sign = if value.minor < 0 { "-" } else { "" };
    let abs = value.minor.unsigned_abs();
    match value.currency {
        Currency::Usd => format!("{sign}${}.{:02}", group_digits(abs / 100), abs % 100),
        Currency::Rub => format!("{sign}{}{NBSP}₽", group_digits((abs + 50) / 100)),
    }
}

pub fn money_compact(value: Money) -> String {
    match value.currency {
        Currency::Usd if value.minor % 100 == 0 => {
            let sign = if value.minor < 0 { "-" } else { "" };
            format!("{sign}${}", group_digits(value.minor.unsigned_abs() / 100))
        }
        _ => money(value),
    }
}

pub fn stars(value: Stars) -> String {
    let sign = if value.0 < 0 { "-" } else { "" };
    let n = value.0.unsigned_abs();
    format!(
        "{sign}{}{NBSP}{}",
        group_digits(n),
        plural_ru(n, "звезда", "звезды", "звёзд")
    )
}

pub fn days(n: u32) -> String {
    format!(
        "{}{NBSP}{}",
        group_digits(n as u64),
        plural_ru(n as u64, "день", "дня", "дней")
    )
}

pub fn traffic(bytes: u64) -> String {
    let (unit_bytes, unit) = if bytes >= TIB {
        (TIB, "ТБ")
    } else {
        (GIB, "ГБ")
    };
    let tenths = (bytes as u128 * 20 + unit_bytes as u128) / (unit_bytes as u128 * 2);
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if whole >= 100 || fraction == 0 {
        let rounded = (bytes as u128 * 2 + unit_bytes as u128) / (unit_bytes as u128 * 2);
        format!("{}{NBSP}{unit}", group_digits(rounded as u64))
    } else {
        format!("{whole},{fraction}{NBSP}{unit}")
    }
}

pub fn group_digits_in(n: u64, locale: Locale) -> String {
    match locale {
        Locale::Ru => group_digits(n),
        Locale::En => group_digits(n).replace(NBSP, ","),
    }
}

pub fn plural_en<'a>(n: u64, one: &'a str, other: &'a str) -> &'a str {
    if n == 1 { one } else { other }
}

pub fn count_in(n: u64, locale: Locale, ru: [&str; 3], en: [&str; 2]) -> String {
    let word = match locale {
        Locale::Ru => plural_ru(n, ru[0], ru[1], ru[2]),
        Locale::En => plural_en(n, en[0], en[1]),
    };
    format!("{}{NBSP}{word}", group_digits_in(n, locale))
}

pub fn money_in(value: Money, locale: Locale) -> String {
    if locale == Locale::Ru {
        return money(value);
    }
    let sign = if value.minor < 0 { "-" } else { "" };
    let abs = value.minor.unsigned_abs();
    match value.currency {
        Currency::Usd => format!(
            "{sign}${}.{:02}",
            group_digits_in(abs / 100, locale),
            abs % 100
        ),
        Currency::Rub => format!("{sign}₽{}", group_digits_in((abs + 50) / 100, locale)),
    }
}

pub fn money_exact_in(value: Money, locale: Locale) -> String {
    if value.currency != Currency::Rub || value.minor % 100 == 0 {
        return money_in(value, locale);
    }
    let sign = if value.minor < 0 { "-" } else { "" };
    let abs = value.minor.unsigned_abs();
    let whole = group_digits_in(abs / 100, locale);
    match locale {
        Locale::Ru => format!("{sign}{whole},{:02}{NBSP}₽", abs % 100),
        Locale::En => format!("{sign}₽{whole}.{:02}", abs % 100),
    }
}

pub fn money_exact(value: Money) -> String {
    money_exact_in(value, Locale::Ru)
}

pub fn money_compact_in(value: Money, locale: Locale) -> String {
    match value.currency {
        Currency::Usd if value.minor % 100 == 0 => {
            let sign = if value.minor < 0 { "-" } else { "" };
            format!(
                "{sign}${}",
                group_digits_in(value.minor.unsigned_abs() / 100, locale)
            )
        }
        _ => money_in(value, locale),
    }
}

pub fn stars_in(value: Stars, locale: Locale) -> String {
    let sign = if value.0 < 0 { "-" } else { "" };
    format!(
        "{sign}{}",
        count_in(
            value.0.unsigned_abs(),
            locale,
            ["звезда", "звезды", "звёзд"],
            ["star", "stars"],
        )
    )
}

pub fn days_in(n: u32, locale: Locale) -> String {
    count_in(
        u64::from(n),
        locale,
        ["день", "дня", "дней"],
        ["day", "days"],
    )
}

pub fn months_in(n: u32, locale: Locale) -> String {
    count_in(
        u64::from(n),
        locale,
        ["месяц", "месяца", "месяцев"],
        ["month", "months"],
    )
}

pub fn devices_in(n: u32, locale: Locale) -> String {
    count_in(
        u64::from(n),
        locale,
        ["устройство", "устройства", "устройств"],
        ["device", "devices"],
    )
}

pub fn term_in(days: u32, locale: Locale) -> String {
    if days >= 28 {
        months_in(crate::tariff::months_for_days(days), locale)
    } else {
        days_in(days, locale)
    }
}

pub fn traffic_in(bytes: u64, locale: Locale) -> String {
    match locale {
        Locale::Ru => traffic(bytes),
        Locale::En => traffic(bytes)
            .replace("ТБ", "TB")
            .replace("ГБ", "GB")
            .replace(',', "."),
    }
}

pub fn gigabytes_in(gb: u64, locale: Locale) -> String {
    let unit = match locale {
        Locale::Ru => "ГБ",
        Locale::En => "GB",
    };
    format!("{}{NBSP}{unit}", group_digits_in(gb, locale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn точная_сумма_показывает_копейки_только_когда_они_есть() {
        let rub = |minor| Money::new(minor, Currency::Rub);
        assert_eq!(money_exact(rub(2_500)), "25\u{a0}₽");
        assert_eq!(money_exact(rub(535)), "5,35\u{a0}₽");
        assert_eq!(money_exact(rub(-100_505)), "-1\u{a0}005,05\u{a0}₽");
        assert_eq!(money_exact_in(rub(535), Locale::En), "₽5.35");
        assert_eq!(money_exact(Money::new(535, Currency::Usd)), "$5.35");
    }

    #[test]
    fn дата_словами_на_двух_языках() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        assert_eq!(date_in(date, Locale::Ru), "24\u{a0}ноября 2026");
        assert_eq!(date_in(date, Locale::En), "November\u{a0}24, 2026");
    }

    #[test]
    fn по_английски_деньги_дни_и_трафик() {
        let en = Locale::En;
        assert_eq!(
            money_in(Money::new(150_100, Currency::Usd), en),
            "$1,501.00"
        );
        assert_eq!(
            money_compact_in(Money::new(150_100, Currency::Usd), en),
            "$1,501"
        );
        assert_eq!(money_in(Money::new(169_000, Currency::Rub), en), "₽1,690");
        assert_eq!(money_in(Money::new(-420, Currency::Usd), en), "-$4.20");
        assert_eq!(
            money_in(Money::new(169_000, Currency::Rub), Locale::Ru),
            "1\u{a0}690\u{a0}₽"
        );
        assert_eq!(days_in(1, en), "1\u{a0}day");
        assert_eq!(days_in(30, en), "30\u{a0}days");
        assert_eq!(days_in(3, Locale::Ru), "3\u{a0}дня");
        assert_eq!(months_in(12, en), "12\u{a0}months");
        assert_eq!(devices_in(1, en), "1\u{a0}device");
        assert_eq!(stars_in(Stars(1200), en), "1,200\u{a0}stars");
        assert_eq!(traffic_in(46 * GIB + GIB / 5, en), "46.2\u{a0}GB");
        assert_eq!(traffic_in(1200 * GIB, en), "1.2\u{a0}TB");
        assert_eq!(gigabytes_in(50, en), "50\u{a0}GB");
        assert_eq!(gigabytes_in(50, Locale::Ru), "50\u{a0}ГБ");
    }

    fn nb(s: &str) -> String {
        s.replace(' ', "\u{a0}")
    }

    #[test]
    fn суммы_как_в_макетах() {
        assert_eq!(money(Money::new(699, Currency::Usd)), "$6.99");
        assert_eq!(money(Money::new(150_100, Currency::Usd)), nb("$1 501.00"));
        assert_eq!(
            money_compact(Money::new(150_100, Currency::Usd)),
            nb("$1 501")
        );
        assert_eq!(money_compact(Money::new(23_670, Currency::Usd)), "$236.70");
        assert_eq!(money(Money::new(49_900, Currency::Rub)), nb("499 ₽"));
        assert_eq!(money(Money::new(169_000, Currency::Rub)), nb("1 690 ₽"));
        assert_eq!(money(Money::new(-420, Currency::Usd)), "-$4.20");
    }

    #[test]
    fn звёзды_склоняются() {
        assert_eq!(stars(Stars(150)), nb("150 звёзд"));
        assert_eq!(stars(Stars(1200)), nb("1 200 звёзд"));
        assert_eq!(stars(Stars(21)), nb("21 звезда"));
        assert_eq!(stars(Stars(2)), nb("2 звезды"));
        assert_eq!(stars(Stars(11)), nb("11 звёзд"));
        assert_eq!(stars(Stars(112)), nb("112 звёзд"));
    }

    #[test]
    fn дни_склоняются() {
        assert_eq!(days(1), nb("1 день"));
        assert_eq!(days(3), nb("3 дня"));
        assert_eq!(days(90), nb("90 дней"));
    }

    #[test]
    fn трафик_в_гигабайтах() {
        assert_eq!(traffic(300 * GIB), nb("300 ГБ"));
        assert_eq!(traffic(112 * GIB), nb("112 ГБ"));
        assert_eq!(traffic(46 * GIB + GIB / 5), nb("46,2 ГБ"));
        assert_eq!(traffic(GIB * 4 / 5), nb("0,8 ГБ"));
        assert_eq!(traffic(6 * TIB + TIB / 5), nb("6,2 ТБ"));
        assert_eq!(traffic(0), nb("0 ГБ"));
    }
}
