use chrono::{DateTime, Datelike, FixedOffset, Utc};
use sn_domain::client::Locale;
use sn_domain::money::{Amount, Money, Stars};
use sn_domain::subscription::{Subscription, SubscriptionKind, SubscriptionStatus};
use sn_domain::{errors_en, format, lang};
use sn_modules::ModuleError;
use sn_services::ServiceError;
use sn_store::StoreError;

const MONTHS: [&str; 12] = [
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

const MONTHS_EN: [&str; 12] = [
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

pub const NB: char = format::NBSP;

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

fn month(index: u32) -> &'static str {
    let index = index as usize;
    match lang::current() {
        Locale::Ru => MONTHS[index],
        Locale::En => MONTHS_EN[index],
    }
}

pub fn date(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    format!(
        "{}{NB}{}{NB}{}",
        local.day(),
        month(local.month0()),
        local.year()
    )
}

pub fn day_month(at: DateTime<Utc>, offset: FixedOffset) -> String {
    let local = at.with_timezone(&offset);
    format!("{}{NB}{}", local.day(), month(local.month0()))
}

pub fn days_left(expire_at: DateTime<Utc>, now: DateTime<Utc>) -> u32 {
    let seconds = (expire_at - now).num_seconds();
    if seconds <= 0 {
        0
    } else {
        u32::try_from((seconds + 86_399) / 86_400).unwrap_or(u32::MAX)
    }
}

pub fn money(value: Money) -> String {
    format::money_in(value, lang::current())
}

pub fn money_exact(value: Money) -> String {
    format::money_exact_in(value, lang::current())
}

pub fn money_signed(value: Money) -> String {
    let shown = money(value);
    match shown.strip_prefix('-') {
        Some(rest) => format!("\u{2212}{rest}"),
        None => shown,
    }
}

pub fn term(days: u32) -> String {
    format::term_in(days, lang::current())
}

pub fn money_compact(value: Money) -> String {
    format::money_compact_in(value, lang::current())
}

pub fn stars(value: Stars) -> String {
    format::stars_in(value, lang::current())
}

pub fn days(n: u32) -> String {
    format::days_in(n, lang::current())
}

pub fn traffic(bytes: u64) -> String {
    format::traffic_in(bytes, lang::current())
}

pub fn devices(n: u32) -> String {
    format::devices_in(n, lang::current())
}

pub fn devices_from(n: u32) -> String {
    format::count_in(
        u64::from(n),
        lang::current(),
        ["доп. устройства", "доп. устройств", "доп. устройств"],
        ["extra device", "extra devices"],
    )
}

pub fn amount(value: Amount) -> String {
    match (value.as_money(), value.as_stars()) {
        (Some(value), _) => money(value),
        (None, Some(value)) => stars(value),
        (None, None) => value.to_string(),
    }
}

pub fn status_key(subscription: &Subscription, now: DateTime<Utc>) -> &'static str {
    let status = subscription.mirror.status.or_else(|| {
        subscription.mirror.expire_at.map(|at| {
            if at <= now {
                SubscriptionStatus::Expired
            } else {
                SubscriptionStatus::Active
            }
        })
    });
    match status {
        Some(SubscriptionStatus::Active) if subscription.kind == SubscriptionKind::Trial => {
            "status.trial"
        }
        Some(SubscriptionStatus::Active) => "status.active",
        Some(SubscriptionStatus::Limited) => "status.limited",
        Some(SubscriptionStatus::Expired) => "status.expired",
        Some(SubscriptionStatus::Disabled) => "status.disabled",
        None => "status.pending",
    }
}

pub fn status_mark(subscription: &Subscription, now: DateTime<Utc>) -> &'static str {
    match status_key(subscription, now) {
        "status.active" | "status.trial" => "🟢",
        _ => "🔴",
    }
}

pub fn plural_days(n: u32) -> String {
    days(n)
}

fn russian_error(err: &ServiceError) -> String {
    match err {
        ServiceError::Rejected { message, .. } => capitalize(message),
        ServiceError::Domain(e) => capitalize(&e.to_string()),
        ServiceError::Module(ModuleError::Invalid(message)) => capitalize(message),
        ServiceError::Module(ModuleError::Disabled(_)) => "Эта функция сейчас выключена.".into(),
        ServiceError::NotImplemented(_)
        | ServiceError::Module(ModuleError::NotImplemented(_))
        | ServiceError::Payment(sn_payments::PaymentError::NotImplemented(_)) => {
            "Эта функция пока недоступна. Попробуйте позже или напишите в поддержку.".into()
        }
        ServiceError::Forbidden => "Аккаунт заблокирован. Напишите в поддержку.".into(),
        ServiceError::InsufficientFunds | ServiceError::Store(StoreError::InsufficientFunds) => {
            "На балансе не хватает средств.".into()
        }
        ServiceError::NotFound(what) | ServiceError::Store(StoreError::NotFound(what)) => {
            format!("Не найдено: {what}.")
        }
        ServiceError::RemnaNotConfigured(_) | ServiceError::Remna(_) => {
            "Сервис VPN сейчас недоступен. Попробуйте через несколько минут.".into()
        }
        ServiceError::Payment(sn_payments::PaymentError::EmailRequired) => {
            "Для этого способа оплаты нужна почта. Выберите другой способ оплаты.".into()
        }
        ServiceError::Payment(_) => {
            "Не получилось выставить счёт. Попробуйте другой способ оплаты или повторите позже."
                .into()
        }
        other => {
            tracing::error!(error = %other, "ошибка в боте");
            "Что-то пошло не так. Попробуйте ещё раз.".into()
        }
    }
}

fn english_error(err: &ServiceError) -> Option<String> {
    match err {
        ServiceError::Rejected { code, .. } => errors_en::message(code),
        ServiceError::Domain(e) => errors_en::message(e.code()),
        ServiceError::Module(ModuleError::Invalid(message)) => Some(capitalize(message)),
        ServiceError::Forbidden => Some("Your account is blocked. Please contact support.".into()),
        ServiceError::NotFound(_) | ServiceError::Store(StoreError::NotFound(_)) => {
            Some("Not found.".into())
        }
        ServiceError::Payment(sn_payments::PaymentError::NotImplemented(_)) => {
            errors_en::message("not_implemented")
        }
        other => errors_en::message(other.code()),
    }
}

pub fn user_error(err: &ServiceError) -> String {
    let russian = russian_error(err);
    match lang::current() {
        Locale::Ru => russian,
        Locale::En => english_error(err).unwrap_or(russian),
    }
}

pub fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

pub fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn даты_по_русски_с_неразрывными_пробелами() {
        let at = Utc.with_ymd_and_hms(2027, 2, 14, 10, 0, 0).unwrap();
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(date(at, utc), "14\u{a0}февраля\u{a0}2027");
        assert_eq!(day_month(at, utc), "14\u{a0}февраля");
        let late = Utc.with_ymd_and_hms(2027, 2, 14, 22, 0, 0).unwrap();
        let msk = FixedOffset::east_opt(3 * 3600).unwrap();
        assert_eq!(day_month(late, msk), "15\u{a0}февраля");
    }

    #[test]
    fn скидка_пишется_тем_же_минусом_что_и_процент() {
        let discount = Money {
            minor: -100,
            currency: sn_domain::money::Currency::Usd,
        };
        assert_eq!(money_signed(discount), "\u{2212}$1.00");
        let price = Money {
            minor: 499,
            currency: sn_domain::money::Currency::Usd,
        };
        assert_eq!(money_signed(price), "$4.99");
        assert_eq!(term(30), "1\u{a0}месяц");
    }

    #[test]
    fn строчной_становится_только_первая_буква() {
        assert_eq!(
            lowercase_first("Перенос в Remnawave и обратно"),
            "перенос в Remnawave и обратно"
        );
        assert_eq!(lowercase_first("Ноды"), "ноды");
        assert_eq!(lowercase_first(""), "");
    }

    #[test]
    fn экранирование_html() {
        assert_eq!(
            escape("<b>A&B</b> \"x\""),
            "&lt;b&gt;A&amp;B&lt;/b&gt; &quot;x&quot;"
        );
    }

    #[test]
    fn даты_и_ошибки_по_английски_внутри_хода() {
        let at = Utc.with_ymd_and_hms(2027, 2, 14, 10, 0, 0).unwrap();
        let utc = FixedOffset::east_opt(0).unwrap();
        lang::sync_scope(Locale::En, || {
            assert_eq!(date(at, utc), "14\u{a0}February\u{a0}2027");
            assert_eq!(day_month(at, utc), "14\u{a0}February");
            assert_eq!(days(2), "2\u{a0}days");
            assert_eq!(
                user_error(&ServiceError::rejected(
                    "trial.used",
                    "Пробный период уже был использован"
                )),
                "You've already used the free trial."
            );
            assert_eq!(
                user_error(&ServiceError::rejected("никто.не.знает", "Счёт устарел")),
                "Счёт устарел"
            );
            assert_eq!(
                user_error(&ServiceError::Forbidden),
                "Your account is blocked. Please contact support."
            );
        });
        assert_eq!(date(at, utc), "14\u{a0}февраля\u{a0}2027");
    }

    #[test]
    fn понятные_ошибки_для_клиента() {
        assert_eq!(
            user_error(&ServiceError::rejected("x", "Счёт устарел")),
            "Счёт устарел"
        );
        assert_eq!(
            user_error(&ServiceError::Module(ModuleError::Invalid(
                "промокод закончился".into()
            ))),
            "Промокод закончился"
        );
        assert!(user_error(&ServiceError::NotImplemented("x")).starts_with("Эта функция пока"));
    }
}
