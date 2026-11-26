use super::model::*;
use super::table::{Dump, Row, minor_from_float};
use sn_domain::money::Currency;
use sn_domain::subscription::SubscriptionKind;
use sn_domain::tariff::{DeviceDiscount, MAX_DEVICE_DISCOUNT_PERCENT, MAX_DEVICE_DISCOUNTS};
use sn_domain::traffic::TrafficReset;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const TABLES: &[&str] = &[
    "clients",
    "tariff_categories",
    "tariffs",
    "tariff_price_options",
    "subscriptions",
    "payments",
    "promo_codes",
    "promo_code_usages",
    "promo_groups",
    "tickets",
    "ticket_messages",
    "system_settings",
    "client_trial_usages",
    "withdrawal_requests",
    "proxy_slots",
    "singbox_slots",
    "gift_codes",
    "admins",
    "auto_broadcast_rules",
    "broadcast_history",
    "gift_history",
    "contests",
    "roulette_spins",
    "roulette_tickets",
];

pub fn looks_like(dump: &Dump) -> bool {
    dump.table("clients")
        .is_some_and(|t| t.has("telegram_id") && t.has("balance"))
        || dump.table("tariff_price_options").is_some()
}

fn currency_of(raw: Option<&str>) -> Option<Currency> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "usd" | "$" => Some(Currency::Usd),
        "rub" | "rur" | "₽" => Some(Currency::Rub),
        _ => None,
    }
}

fn settings(dump: &Dump) -> HashMap<String, String> {
    dump.rows("system_settings")
        .filter_map(|r| Some((r.string("key")?, r.raw("value").unwrap_or("").to_string())))
        .collect()
}

fn provider_of(raw: &str) -> String {
    if let Some(known) = old_provider(raw) {
        return known.into();
    }
    match raw.trim().to_ascii_lowercase().as_str() {
        "lava" => "lava_business".into(),
        "" => "unknown".into(),
        other => other.to_string(),
    }
}

pub fn old_provider(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "yookassa" => Some("yookassa"),
        "platega" | "platega_mir" | "platega-mir" => Some("platega"),
        "lavatop" | "lava_top" | "lava-top" => Some("lava"),
        "heleket" => Some("heleket"),
        "cryptobot" | "crypto_pay" | "cryptopay" => Some("cryptopay"),
        "rollypay" => Some("rollypay"),
        "yoomoney" | "yoomoney_form" => Some("yoomoney"),
        "telegram_stars" | "stars" => Some("stars"),
        "cloudpayments" => Some("cloudpayments"),
        "overpay" => Some("overpay"),
        "cispay" | "cis_pay" | "cis-pay" => Some("cispay"),
        "paritypay" | "parity_pay" | "parity-pay" => Some("paritypay"),
        _ => None,
    }
}

pub const INVOICE_DAYS: i64 = 30;

fn paid_status(row: &Row<'_>) -> Option<bool> {
    match row.text("status")?.to_ascii_uppercase().as_str() {
        "PAID" => Some(false),
        "REFUNDED" => Some(true),
        _ => None,
    }
}

fn decide_currency(dump: &Dump, settings: &HashMap<String, String>) -> (Currency, HashSet<String>) {
    let mut sums: HashMap<Currency, i64> = HashMap::new();
    let mut per_client: HashMap<String, HashSet<Currency>> = HashMap::new();
    for row in dump.rows("payments") {
        if paid_status(&row).is_none() {
            continue;
        }
        let provider = row.text("provider").map(provider_of).unwrap_or_default();
        if provider == "stars" || provider == "admin_grant" {
            continue;
        }
        let Some(currency) = currency_of(row.text("currency")) else {
            continue;
        };
        let amount = row.f64("amount").and_then(minor_from_float).unwrap_or(0);
        *sums.entry(currency).or_default() += amount;
        if provider != "balance"
            && let Some(client) = row.string("client_id")
        {
            per_client.entry(client).or_default().insert(currency);
        }
    }
    let setting = settings
        .get("default_currency")
        .and_then(|v| currency_of(Some(v.trim_matches(['"', '[', ']']))));
    let currency = match sums.len() {
        1 => *sums.keys().next().unwrap_or(&Currency::Usd),
        0 => setting.unwrap_or_else(|| {
            let mut tally: HashMap<Currency, usize> = HashMap::new();
            for row in dump.rows("tariffs") {
                if let Some(c) = currency_of(row.text("currency")) {
                    *tally.entry(c).or_default() += 1;
                }
            }
            tally
                .into_iter()
                .max_by_key(|(_, n)| *n)
                .map_or(Currency::Usd, |(c, _)| c)
        }),
        _ => setting.unwrap_or_else(|| {
            sums.iter()
                .max_by_key(|(_, sum)| **sum)
                .map_or(Currency::Usd, |(c, _)| *c)
        }),
    };
    let mixed = per_client
        .into_iter()
        .filter(|(_, set)| set.len() > 1)
        .map(|(client, _)| client)
        .collect();
    (currency, mixed)
}

fn percent(value: Option<f64>) -> Option<i32> {
    value
        .filter(|v| (0.0..=100.0).contains(v))
        .map(|v| v.round() as i32)
}

fn clients(dump: &Dump, legacy: &mut Legacy, mixed: &HashSet<String>) {
    let trial_usage: HashSet<String> = dump
        .rows("client_trial_usages")
        .filter_map(|r| r.string("client_id"))
        .collect();
    let mut seen_tg = HashSet::new();
    for row in dump.rows("clients") {
        let Some(old_id) = row.string("id") else {
            continue;
        };
        if row
            .text("role")
            .is_some_and(|r| !r.eq_ignore_ascii_case("CLIENT"))
        {
            continue;
        }
        let telegram_id = row.i64("telegram_id").filter(|id| *id > 0);
        if let Some(tg) = telegram_id
            && !seen_tg.insert(tg)
        {
            legacy.problem(
                "duplicate_telegram",
                "client",
                Some(old_id.clone()),
                Some(format!("Telegram {tg}")),
                "Этот Telegram ID уже есть у другого клиента в выгрузке — клиент пропущен",
            );
            continue;
        }
        let balance = row
            .f64("balance")
            .and_then(minor_from_float)
            .unwrap_or(0)
            .max(0);
        let is_mixed = mixed.contains(&old_id) && balance > 0;
        let utm = Utm {
            source: row.string("utm_source"),
            medium: row.string("utm_medium"),
            campaign: row.string("utm_campaign"),
            content: row.string("utm_content"),
            term: row.string("utm_term"),
        };
        let has_utm = utm.source.is_some()
            || utm.medium.is_some()
            || utm.campaign.is_some()
            || utm.content.is_some()
            || utm.term.is_some();
        legacy.clients.push(LClient {
            telegram_id,
            username: clean_username(row.text("telegram_username")),
            first_name: None,
            last_name: None,
            email: clean_email(row.text("email")),
            email_raw: clean_email(row.text("email")).and(row.string("email")),
            email_verified: true,
            password_hash: password_hash(row.text("password_hash")),
            google_id: row.string("google_id"),
            apple_id: row.string("apple_id"),
            locale: locale_of(row.text("preferred_lang")),
            blocked: row.flag("is_blocked"),
            block_reason: row.string("block_reason"),
            bot_unreachable: row.flag("telegram_unreachable"),
            balance_minor: if is_mixed { 0 } else { balance },
            balance_mixed: is_mixed,
            referral_code: row.string("referral_code"),
            referrer_old_id: row.string("referrer_id"),
            referral_percent: percent(row.f64("referral_percent")),
            utm: has_utm.then_some(utm),
            trial_used: row.flag("trial_used") || trial_usage.contains(&old_id),
            created_at: row.time("created_at"),
            old_id,
        });
        bad_email(legacy, row.text("email"));
        if is_mixed {
            let last = legacy.clients.last().map(LClient::who);
            legacy.problem(
                "balance_mixed",
                "client",
                legacy.clients.last().map(|c| c.old_id.clone()),
                last,
                format!(
                    "Баланс {} копился и в рублях, и в долларах — не перенесён, решите вручную",
                    format_minor(balance)
                ),
            );
        }
    }
    let mixed_count = legacy.clients.iter().filter(|c| c.balance_mixed).count() as u64;
    legacy.skip(
        "balance_mixed",
        mixed_count,
        "клиентов пополняли баланс и в рублях, и в долларах. Такой баланс не переносим — суммы в отчёте, решите вручную.",
    );
}

pub fn format_minor(minor: i64) -> String {
    format!("{}.{:02}", minor / 100, (minor % 100).abs())
}

fn uuids(values: Vec<String>) -> Vec<Uuid> {
    values
        .iter()
        .filter_map(|v| Uuid::parse_str(v.trim()).ok())
        .collect()
}

fn tariffs(dump: &Dump, legacy: &mut Legacy) -> HashMap<String, u32> {
    for row in dump.rows("tariff_categories") {
        let (Some(old_id), Some(name)) = (row.string("id"), row.string("name")) else {
            continue;
        };
        if row.flag("single_subscription_mode") {
            legacy.category_limits.insert(old_id.clone(), 1);
        }
        legacy.categories.push(LCategory {
            old_id,
            name,
            sort: row.i64("sort_order").unwrap_or(0) as i32,
        });
    }
    let mut option_days = HashMap::new();
    let mut options: HashMap<String, Vec<LPeriod>> = HashMap::new();
    for row in dump.rows("tariff_price_options") {
        let (Some(old_id), Some(tariff), Some(days)) = (
            row.string("id"),
            row.string("tariff_id"),
            row.i64("duration_days"),
        ) else {
            continue;
        };
        let Ok(days) = u32::try_from(days) else {
            continue;
        };
        if days == 0 {
            continue;
        }
        option_days.insert(old_id.clone(), days);
        options.entry(tariff).or_default().push(LPeriod {
            old_id: Some(old_id),
            days,
            price_minor: row.f64("price").and_then(minor_from_float),
            sort: row.i64("sort_order").unwrap_or(0) as i32,
        });
    }
    for row in dump.rows("tariffs") {
        let (Some(old_id), Some(category), Some(name)) = (
            row.string("id"),
            row.string("category_id"),
            row.string("name"),
        ) else {
            continue;
        };
        let currency = currency_of(row.text("currency")).unwrap_or(legacy.currency);
        let mut periods = options.remove(&old_id).unwrap_or_default();
        if periods.is_empty()
            && let Some(days) = row.i64("duration_days").and_then(|d| u32::try_from(d).ok())
            && days > 0
        {
            periods.push(LPeriod {
                old_id: None,
                days,
                price_minor: row.f64("price").and_then(minor_from_float),
                sort: 0,
            });
        }
        let mut seen = HashSet::new();
        periods.retain(|p| seen.insert(p.days));
        let raw_reset = row.text("traffic_reset_mode");
        let reset = parse_reset(raw_reset).unwrap_or(TrafficReset::NoReset);
        let mut notes = Vec::new();
        if raw_reset.is_some_and(|r| r.eq_ignore_ascii_case("carry_over")) {
            notes.push(
                "перенос остатка трафика в новой системе нет — стоит «Без сброса»".to_string(),
            );
        }
        if currency != legacy.currency {
            notes.push(format!(
                "цены тарифа в {}, а основная валюта {} — тариф снят с продажи",
                currency.code(),
                legacy.currency.code()
            ));
        }
        let extra_max = row
            .i64("max_extra_devices")
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);
        let extra_price = row
            .f64("price_per_extra_device")
            .and_then(minor_from_float)
            .filter(|v| *v > 0);
        let tiers = row.json("device_discount_tiers");
        if extra_price.is_some() && extra_max > 0 {
            let (steps, dropped) = device_ladder(tiers, extra_max);
            notes.extend(dropped);
            if !steps.is_empty() {
                legacy.device_discounts.insert(old_id.clone(), steps);
            }
        } else if tiers
            .as_ref()
            .and_then(|t| t.as_array())
            .is_some_and(|t| !t.is_empty())
        {
            notes.push(
                "скидки за доп. устройства не перенесены: у тарифа нет доп. устройств, в старой панели они не применялись"
                    .to_string(),
            );
        }
        legacy.tariffs.push(LTariff {
            category_old_id: category,
            name,
            description: row.string("description"),
            sort: row.i64("sort_order").unwrap_or(0) as i32,
            on_sale: currency == legacy.currency,
            currency,
            periods,
            devices_included: row
                .i64("included_devices")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(1),
            extra_devices_max: if extra_price.is_some() { extra_max } else { 0 },
            extra_device_monthly_minor: extra_price.filter(|_| extra_max > 0),
            traffic_limit_bytes: row
                .i64("traffic_limit_bytes")
                .and_then(|v| u64::try_from(v).ok())
                .filter(|v| *v > 0),
            traffic_reset: reset,
            squads: uuids(row.list("internal_squad_uuids")),
            external_squad: None,
            note: (!notes.is_empty()).then(|| notes.join("; ")),
            old_id,
        });
    }
    option_days
}

pub fn device_ladder(
    raw: Option<serde_json::Value>,
    max_extra: u32,
) -> (Vec<DeviceDiscount>, Vec<String>) {
    let mut raw = raw;
    if let Some(serde_json::Value::String(text)) = &raw {
        raw = serde_json::from_str(text).ok();
    }
    let mut dropped = Vec::new();
    let mut unreadable = 0usize;
    let mut steps: Vec<(u32, u8)> = Vec::new();
    for item in raw
        .as_ref()
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let number = |key: &str| item.get(key).and_then(serde_json::Value::as_f64);
        let from = number("minExtraDevices").or_else(|| number("minDevices"));
        let percent = number("discountPercent");
        let (Some(from), Some(percent)) = (from, percent) else {
            unreadable += 1;
            continue;
        };
        if !from.is_finite() || !percent.is_finite() || percent < 0.0 {
            unreadable += 1;
            continue;
        }
        let from = from.floor().clamp(0.0, f64::from(u32::MAX)) as u32;
        let percent = percent.round().min(255.0) as u8;
        steps.push((from, percent));
    }
    if unreadable > 0 {
        dropped.push(format!(
            "ступеней без числа устройств или процента не перенесено: {unreadable}"
        ));
    }
    steps.sort_by_key(|(from, percent)| (*from, std::cmp::Reverse(*percent)));
    let mut ladder: Vec<DeviceDiscount> = Vec::new();
    let mut seen: Option<u32> = None;
    for (from, percent) in steps {
        let step = format!("ступень скидки от {from} доп. устройств ({percent} %) не перенесена");
        if seen == Some(from) {
            dropped.push(format!(
                "{step}: от того же числа устройств оставлена большая скидка"
            ));
            continue;
        }
        seen = Some(from);
        if from == 0 {
            dropped.push(format!(
                "{step}: скидка начинается хотя бы с одного доп. устройства"
            ));
            continue;
        }
        if from > max_extra {
            dropped.push(format!(
                "{step}: докупить можно не больше {max_extra} доп. устройств"
            ));
            continue;
        }
        if percent == 0 || percent > MAX_DEVICE_DISCOUNT_PERCENT {
            dropped.push(format!(
                "{step}: скидка — от 1 до {MAX_DEVICE_DISCOUNT_PERCENT} %"
            ));
            continue;
        }
        if ladder.last().is_some_and(|d| percent <= d.percent) {
            dropped.push(format!(
                "{step}: в новой панели скидка растёт с числом устройств"
            ));
            continue;
        }
        if ladder.len() == MAX_DEVICE_DISCOUNTS {
            dropped.push(format!(
                "{step}: ступеней может быть не больше {MAX_DEVICE_DISCOUNTS}"
            ));
            continue;
        }
        ladder.push(DeviceDiscount { from, percent });
    }
    (ladder, dropped)
}

fn subscriptions(dump: &Dump, legacy: &mut Legacy) {
    let coded = parked_by_code(dump);
    let mut gifts = 0u64;
    let mut hidden = 0u64;
    let now = legacy.dumped_at.unwrap_or_else(chrono::Utc::now);
    let mut with_sub: HashSet<String> = HashSet::new();
    for row in dump.rows("subscriptions") {
        let (Some(old_id), Some(owner)) = (row.string("id"), row.string("owner_id")) else {
            continue;
        };
        if row.text("gift_status").is_some_and(|s| {
            matches!(
                s.to_ascii_uppercase().as_str(),
                "GIFT_RESERVED" | "GIFT_CODE_ACTIVE"
            )
        }) {
            if !coded.contains(&old_id) {
                gifts += 1;
            }
            continue;
        }
        with_sub.insert(owner.clone());
        if row.text("hidden_at").is_some() && row.time("expire_at").is_none_or(|at| at < now) {
            hidden += 1;
            continue;
        }
        let raw_remna = row.string("remnawave_uuid");
        let remna_id = raw_remna.as_deref().and_then(|v| v.parse::<i64>().ok());
        let kind = if row.text("trial_id").is_some() {
            SubscriptionKind::Trial
        } else if row.flag("promo_granted") {
            SubscriptionKind::Promo
        } else if row.flag("purchased_as_gift") {
            SubscriptionKind::Gift
        } else if row.text("tariff_id").is_none() && row.text("custom_price").is_some() {
            SubscriptionKind::Custom
        } else {
            SubscriptionKind::Paid
        };
        legacy.subscriptions.push(LSubscription {
            client_old_id: owner,
            tariff_old_id: row.string("tariff_id"),
            kind,
            remna_id,
            remna_uuid: raw_remna.filter(|_| remna_id.is_none()),
            short_uuid: row.string("short_uuid"),
            expire_at: row.time("expire_at"),
            extra_devices: row
                .i64("extra_devices")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0),
            autorenew: row.flag("auto_renew_enabled"),
            autorenew_days: None,
            autorenew_period_old_id: row.string("auto_renew_price_option_id"),
            ordinal: row
                .i64("subscription_index")
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0),
            gifted: row
                .text("gift_status")
                .is_some_and(|s| s.eq_ignore_ascii_case("GIFTED")),
            created_at: row.time("created_at"),
            old_id,
        });
    }
    for row in dump.rows("clients") {
        let Some(id) = row.string("id") else { continue };
        if with_sub.contains(&id) {
            continue;
        }
        let Some(raw) = row.string("remnawave_uuid") else {
            continue;
        };
        let remna_id = raw.parse::<i64>().ok();
        legacy.subscriptions.push(LSubscription {
            old_id: format!("client:{id}"),
            client_old_id: id,
            tariff_old_id: row.string("current_tariff_id"),
            kind: SubscriptionKind::Paid,
            remna_id,
            remna_uuid: (remna_id.is_none()).then_some(raw),
            short_uuid: None,
            expire_at: None,
            extra_devices: 0,
            autorenew: row.flag("auto_renew_enabled"),
            autorenew_days: None,
            autorenew_period_old_id: row.string("auto_renew_price_option_id"),
            ordinal: 0,
            gifted: false,
            created_at: row.time("created_at"),
        });
    }
    legacy.skip(
        "gift_unclaimed",
        gifts,
        "подарочных подписок отложены без действующего кода. Их нет у получателей — после переключения выдайте вручную.",
    );
    legacy.skip(
        "subscription_hidden",
        hidden,
        "давно истёкших подписок старая панель уже скрыла от клиентов. Не переносим.",
    );
}

fn payments(dump: &Dump, legacy: &mut Legacy, option_days: &HashMap<String, u32>) {
    let mut unpaid = 0u64;
    let mut free = 0u64;
    let mut unknown_currency = 0u64;
    for row in dump.rows("payments") {
        let (Some(old_id), Some(client)) = (row.string("id"), row.string("client_id")) else {
            continue;
        };
        let Some(refunded) = paid_status(&row) else {
            match pending_invoice(&row, legacy, &old_id, &client, option_days) {
                Some(invoice) => legacy.invoices.push(invoice),
                None => unpaid += 1,
            }
            continue;
        };
        let provider = row
            .text("provider")
            .map(provider_of)
            .unwrap_or_else(|| "unknown".into());
        let meta = row.json("metadata").unwrap_or(serde_json::Value::Null);
        let stars = meta
            .pointer("/stars/total")
            .and_then(serde_json::Value::as_i64)
            .filter(|n| *n > 0);
        let currency = if provider == "stars" && stars.is_some() {
            PayCurrencyLegacy::Stars
        } else if let Some(c) = currency_of(row.text("currency")) {
            PayCurrencyLegacy::Money(c)
        } else {
            unknown_currency += 1;
            continue;
        };
        let amount = match currency {
            PayCurrencyLegacy::Stars => stars.unwrap_or(0),
            PayCurrencyLegacy::Money(_) => {
                row.f64("amount").and_then(minor_from_float).unwrap_or(0)
            }
        };
        if amount <= 0 || provider == "admin_grant" {
            free += 1;
            continue;
        }
        if !refunded && let Some(contract) = lava_contract(&row, &old_id, &client, option_days) {
            legacy.invoices.push(contract);
        }
        let (kind, label) =
            if row.text("tariff_id").is_some() || row.text("tariff_price_option_id").is_some() {
                (PaymentKind::Subscription, None)
            } else if row.text("proxy_tariff_id").is_some() {
                (PaymentKind::Other, Some("Прокси".to_string()))
            } else if row.text("singbox_tariff_id").is_some() {
                (PaymentKind::Other, Some("Sing-box".to_string()))
            } else if meta.get("extraOption").is_some() {
                (PaymentKind::Other, Some("Доп. опция".to_string()))
            } else if meta.get("customBuild").is_some() {
                (PaymentKind::Other, Some("Своя подписка".to_string()))
            } else if provider == "balance" {
                (PaymentKind::Other, Some("Покупка с баланса".to_string()))
            } else {
                (PaymentKind::TopUp, None)
            };
        let days = row
            .text("tariff_price_option_id")
            .and_then(|id| option_days.get(id).copied())
            .or_else(|| {
                meta.pointer("/customBuild/durationDays")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|d| u32::try_from(d).ok())
            });
        legacy.payments.push(LPayment {
            client_old_id: client,
            kind,
            refunded,
            amount_minor: amount,
            currency,
            provider,
            external_id: row.string("external_id"),
            tariff_old_id: row.string("tariff_id"),
            days,
            subscription_old_id: row.string("subscription_id"),
            label,
            created_at: row.time("created_at"),
            paid_at: row.time("paid_at").or_else(|| row.time("created_at")),
            old_id,
        });
    }
    legacy.skip(
        "payments_unpaid",
        unpaid,
        "неоплаченных и отменённых счетов: в истории они не нужны.",
    );
    let open: Vec<&LInvoice> = legacy
        .invoices
        .iter()
        .filter(|i| !i.old_id.starts_with("contract:"))
        .collect();
    let kept = open
        .iter()
        .filter(|i| i.recurring && invoice_stale(legacy, i.created_at))
        .count() as u64;
    let fresh = open.len() as u64 - kept;
    if fresh > 0 {
        legacy.skip(
            "invoices_waiting",
            fresh,
            format!("неоплаченных счетов последних {INVOICE_DAYS} дней не попадут в историю, но новая панель их запомнит: если клиент оплатит такой счёт после переключения, купленное выдастся само."),
        );
    }
    legacy.skip(
        "lava_contracts_kept",
        kept,
        format!("счетов Lava.top старше {INVOICE_DAYS} дней без отметки об оплате: старая панель могла не заметить оплату, а Lava.top продолжает списывать деньги по договору. Новая панель запомнит эти договоры и проведёт такие списания сама."),
    );
    legacy.skip(
        "payments_free",
        free,
        "выдач без оплаты: в истории платежей их нет, подписки переносятся.",
    );
    legacy.skip(
        "payments_currency",
        unknown_currency,
        "оплат в незнакомой валюте: их нет в истории.",
    );
}

fn invoice_stale(legacy: &Legacy, created_at: chrono::DateTime<chrono::Utc>) -> bool {
    legacy.dumped_at.unwrap_or_else(chrono::Utc::now) - created_at
        > chrono::Duration::days(INVOICE_DAYS)
}

fn pending_invoice(
    row: &Row<'_>,
    legacy: &Legacy,
    old_id: &str,
    client: &str,
    option_days: &HashMap<String, u32>,
) -> Option<LInvoice> {
    if !row
        .text("status")
        .is_some_and(|s| s.eq_ignore_ascii_case("PENDING"))
    {
        return None;
    }
    let created_at = row.time("created_at")?;
    let raw_provider = row.string("provider")?;
    let provider = old_provider(&raw_provider).filter(|p| *p != "stars")?;
    let stale = invoice_stale(legacy, created_at);
    if stale && provider != "lava" {
        return None;
    }
    let meta = row.json("metadata").unwrap_or(serde_json::Value::Null);
    let stars = meta
        .pointer("/stars/total")
        .and_then(serde_json::Value::as_i64)
        .filter(|n| *n > 0);
    let currency = currency_of(row.text("currency")).unwrap_or(legacy.currency);
    let amount_minor = row
        .f64("amount")
        .and_then(minor_from_float)
        .filter(|a| *a > 0)?;
    let text = |key: &str| {
        meta.get(key).and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    let purpose = if let Some(tariff) = row.string("tariff_id") {
        if meta
            .get("purchasedAsGift")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            InvoicePurpose::Other("Подарок".into())
        } else {
            let period = row.string("tariff_price_option_id");
            InvoicePurpose::Tariff {
                days: period.as_ref().and_then(|p| option_days.get(p).copied()),
                period_old_id: period,
                subscription_old_id: text("extendsSecondarySubId")
                    .or_else(|| text("targetSubscriptionId"))
                    .or_else(|| row.string("subscription_id")),
                tariff_old_id: tariff,
            }
        }
    } else if meta.get("extraOption").is_some() {
        InvoicePurpose::Other("Доп. опция".into())
    } else if row.text("proxy_tariff_id").is_some() || row.text("singbox_tariff_id").is_some() {
        return None;
    } else {
        InvoicePurpose::TopUp
    };
    Some(LInvoice {
        old_id: old_id.to_string(),
        client_old_id: client.to_string(),
        provider,
        old_provider: raw_provider,
        external_id: row.string("external_id"),
        order_ref: row.string("order_id"),
        amount_minor,
        currency,
        stars,
        purpose,
        pay_url: text("redirectTargetUrl").filter(|u| !stale && u.starts_with("https://")),
        recurring: provider == "lava",
        created_at,
    })
}

fn lava_contract(
    row: &Row<'_>,
    old_id: &str,
    client: &str,
    option_days: &HashMap<String, u32>,
) -> Option<LInvoice> {
    if !row
        .text("provider")
        .is_some_and(|p| p.eq_ignore_ascii_case("lavatop"))
    {
        return None;
    }
    let tariff = row.string("tariff_id")?;
    let order_ref = row.string("order_id")?;
    let period = row.string("tariff_price_option_id");
    Some(LInvoice {
        old_id: format!("contract:{old_id}"),
        client_old_id: client.to_string(),
        provider: "lava",
        old_provider: "lavatop".into(),
        external_id: None,
        order_ref: Some(order_ref),
        amount_minor: row
            .f64("amount")
            .and_then(minor_from_float)
            .filter(|a| *a > 0)?,
        currency: currency_of(row.text("currency"))?,
        stars: None,
        purpose: InvoicePurpose::Tariff {
            days: period.as_ref().and_then(|p| option_days.get(p).copied()),
            period_old_id: period,
            subscription_old_id: row.string("subscription_id"),
            tariff_old_id: tariff,
        },
        pay_url: None,
        recurring: true,
        created_at: row.time("paid_at").or_else(|| row.time("created_at"))?,
    })
}

fn promo_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    let valid = (3..=32).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then_some(code)
}

fn promos(dump: &Dump, legacy: &mut Legacy) {
    let mut bad = 0u64;
    for row in dump.rows("promo_codes") {
        let (Some(old_id), Some(raw)) = (row.string("id"), row.string("code")) else {
            continue;
        };
        let Some(code) = promo_code(&raw) else {
            bad += 1;
            legacy.problem(
                "promo_code_format",
                "promo",
                Some(old_id),
                Some(raw),
                "Код промокода должен быть из латинских букв и цифр, от 3 до 32 знаков — не перенесён",
            );
            continue;
        };
        let kind = row.text("type").unwrap_or("").to_ascii_uppercase();
        let value = if kind == "FREE_DAYS" {
            row.i64("duration_days")
                .and_then(|d| u32::try_from(d).ok())
                .filter(|d| (1..=3650).contains(d))
                .map(PromoValue::Days)
        } else if let Some(p) = row.f64("discount_percent").filter(|p| *p > 0.0) {
            Some(PromoValue::Percent(p.round().clamp(1.0, 100.0) as u8))
        } else {
            row.f64("discount_fixed")
                .and_then(minor_from_float)
                .filter(|v| *v > 0)
                .map(PromoValue::Fixed)
        };
        let Some(value) = value else {
            bad += 1;
            legacy.problem(
                "promo_value",
                "promo",
                Some(old_id),
                Some(code),
                "У промокода нет скидки или дней — не перенесён",
            );
            continue;
        };
        legacy.promos.push(LPromo {
            code,
            value,
            max_uses: row
                .i64("max_uses")
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0),
            per_client: row
                .i64("max_uses_per_client")
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0)
                .unwrap_or(1),
            valid_from: None,
            valid_until: row.time("expires_at"),
            active: row.bool("is_active").unwrap_or(true),
            first_purchase_only: false,
            tariff_old_id: row.string("tariff_id"),
            note: row.string("name"),
            old_id,
        });
    }
    for row in dump.rows("promo_code_usages") {
        let (Some(old_id), Some(promo), Some(client)) = (
            row.string("id"),
            row.string("promo_code_id"),
            row.string("client_id"),
        ) else {
            continue;
        };
        legacy.promo_uses.push(LPromoUse {
            old_id,
            promo_old_id: promo,
            client_old_id: client,
            used_at: row.time("created_at"),
        });
    }
    legacy.skip(
        "promo_invalid",
        bad,
        "промокодов с кодом или скидкой, которые новая система не принимает: список в отчёте.",
    );
    legacy.skip(
        "promo_links",
        dump.count("promo_groups") as u64,
        "промо-ссылок на бесплатные дни: в новой системе их заменяют промокоды на дни.",
    );
}

fn tickets(dump: &Dump, legacy: &mut Legacy) {
    let mut messages: HashMap<String, Vec<LMessage>> = HashMap::new();
    for row in dump.rows("ticket_messages") {
        let (Some(old_id), Some(ticket)) = (row.string("id"), row.string("ticket_id")) else {
            continue;
        };
        let body = row.raw("content").unwrap_or("").trim().to_string();
        let attachments = row
            .json("attachments")
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or(0);
        let body = match (body.is_empty(), attachments) {
            (true, 0) => continue,
            (true, n) => format!("[вложений: {n}, файлы остались в старой панели]"),
            (false, 0) => body,
            (false, n) => format!("{body}\n[вложений: {n}, файлы остались в старой панели]"),
        };
        messages.entry(ticket).or_default().push(LMessage {
            old_id,
            from_support: row
                .text("author_type")
                .is_some_and(|a| a.eq_ignore_ascii_case("support")),
            body,
            read: row.flag("is_read"),
            created_at: row.time("created_at"),
        });
    }
    for row in dump.rows("tickets") {
        let (Some(old_id), Some(client)) = (row.string("id"), row.string("client_id")) else {
            continue;
        };
        let mut list = messages.remove(&old_id).unwrap_or_default();
        list.sort_by_key(|m| m.created_at);
        legacy.tickets.push(LTicket {
            client_old_id: client,
            subject: row.string("subject"),
            open: !row
                .text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("closed")),
            created_at: row.time("created_at"),
            closed_at: row.time("closed_at"),
            messages: list,
            old_id,
        });
    }
}

fn leftovers(dump: &Dump, legacy: &mut Legacy) {
    let slots = (dump.count("proxy_slots") + dump.count("singbox_slots")) as u64;
    legacy.skip(
        "products_absent",
        slots,
        "прокси и sing-box: в новой системе этих товаров нет. Оплаты за них останутся в истории.",
    );
}

fn parked_by_code(dump: &Dump) -> HashSet<String> {
    dump.rows("gift_codes")
        .filter(|r| {
            r.text("status")
                .is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE"))
        })
        .filter_map(|r| r.string("secondary_subscription_id"))
        .collect()
}

fn withdrawals(dump: &Dump, legacy: &mut Legacy) {
    for row in dump.rows("withdrawal_requests") {
        let (Some(old_id), Some(client)) = (row.string("id"), row.string("client_id")) else {
            continue;
        };
        let amount = row.f64("amount").and_then(minor_from_float).unwrap_or(0);
        let state = match row.text("status").map(str::to_ascii_uppercase).as_deref() {
            Some("PENDING") => WithdrawalState::Pending,
            Some("APPROVED" | "PAID" | "COMPLETED") => WithdrawalState::Paid,
            Some("REJECTED" | "DECLINED" | "CANCELED" | "CANCELLED") => WithdrawalState::Rejected,
            _ => continue,
        };
        let details = row
            .string("wallet_trc20")
            .or_else(|| row.string("details"))
            .unwrap_or_default();
        if amount <= 0 || details.is_empty() {
            legacy.problem(
                "withdrawal_broken",
                "client",
                Some(client),
                None,
                format!(
                    "Заявка на вывод {} без суммы или реквизитов — не перенесена",
                    format_minor(amount)
                ),
            );
            continue;
        }
        legacy.withdrawals.push(LWithdrawal {
            old_id,
            client_old_id: client,
            amount_minor: amount,
            details,
            state,
            comment: row.string("admin_comment"),
            created_at: row.time("created_at"),
            decided_at: row.time("processed_at"),
        });
    }
}

fn gifts(dump: &Dump, legacy: &mut Legacy) {
    let now = legacy.dumped_at.unwrap_or_else(chrono::Utc::now);
    let parked: HashMap<String, (Option<String>, Option<chrono::DateTime<chrono::Utc>>)> = dump
        .rows("subscriptions")
        .filter_map(|r| {
            Some((
                r.string("id")?,
                (r.string("tariff_id"), r.time("expire_at")),
            ))
        })
        .collect();
    let mut stale = 0u64;
    for row in dump.rows("gift_codes") {
        if !row
            .text("status")
            .is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE"))
        {
            continue;
        }
        let (Some(old_id), Some(code), Some(giver)) = (
            row.string("id"),
            row.string("code"),
            row.string("creator_id"),
        ) else {
            continue;
        };
        let expires_at = row.time("expires_at");
        let (tariff, until) = row
            .string("secondary_subscription_id")
            .and_then(|s| parked.get(&s).cloned())
            .unwrap_or((None, None));
        let by_tariff = || {
            let tariff = tariff.as_ref()?;
            legacy
                .tariffs
                .iter()
                .find(|t| &t.old_id == tariff)?
                .periods
                .iter()
                .map(|p| p.days)
                .min()
        };
        let days = match until {
            Some(at) => Some((at - now).num_seconds()).filter(|s| *s > 0).map(|s| {
                u32::try_from((s + 86_399) / 86_400)
                    .unwrap_or(u32::MAX)
                    .min(3650)
            }),
            None => by_tariff(),
        };
        match days {
            Some(days) if expires_at.is_none_or(|at| at > now) => {
                legacy.gifts.push(LGift {
                    old_id,
                    code: code.trim().to_ascii_uppercase(),
                    giver_old_id: giver,
                    tariff_old_id: tariff,
                    days,
                    expires_at,
                    created_at: row.time("created_at"),
                });
            }
            _ => stale += 1,
        }
    }
    legacy.skip(
        "gift_codes_stale",
        stale,
        "подарочных кодов уже истекли или их подписка закончилась. Не переносим.",
    );
    let history = dump.count("gift_history") as u64;
    legacy.skip(
        "gift_history",
        history,
        "записей истории подарков. Подаренные подписки уже у получателей, а сама история остаётся в старой панели.",
    );
}

fn admins(dump: &Dump, legacy: &mut Legacy) {
    for row in dump.rows("admins") {
        let (Some(old_id), Some(email)) = (row.string("id"), row.string("email")) else {
            continue;
        };
        let Some(hash) = password_hash(row.text("password_hash")) else {
            legacy.problem(
                "admin_password",
                "admin",
                Some(old_id),
                Some(email),
                "Пароль админа сохранён в непонятном виде — админ не перенесён, создайте его заново",
            );
            continue;
        };
        let full_access = row
            .text("role")
            .is_none_or(|r| r.eq_ignore_ascii_case("ADMIN"));
        let sections = row.list("allowed_sections");
        let sections = if sections.is_empty() {
            row.text("allowed_sections")
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            sections
        };
        legacy.admins.push(LAdmin {
            old_id,
            email: email.trim().to_string(),
            password_hash: hash,
            full_access,
            sections,
        });
    }
}

fn auto_rules(dump: &Dump, legacy: &mut Legacy) {
    for row in dump.rows("auto_broadcast_rules") {
        let (Some(old_id), Some(trigger), Some(text)) = (
            row.string("id"),
            row.string("trigger_type"),
            row.string("message"),
        ) else {
            continue;
        };
        let mut buttons = Vec::new();
        for (label, url) in [
            ("button_text", "button_url"),
            ("button2_text", "button2_url"),
        ] {
            if let (Some(label), Some(url)) = (row.string(label), row.string(url)) {
                buttons.push((label, url));
            }
        }
        legacy.auto_rules.push(LAutoRule {
            old_id,
            name: row.string("name"),
            trigger: trigger.to_ascii_lowercase(),
            days: row.i64("delay_days").unwrap_or(0),
            channel: row
                .string("channel")
                .unwrap_or_else(|| "telegram".into())
                .to_ascii_lowercase(),
            subject: row.string("subject"),
            text,
            buttons,
            enabled: row.flag("enabled"),
        });
    }
    legacy.skip(
        "broadcast_history",
        dump.count("broadcast_history") as u64,
        "разовых рассылок в истории. Они уже отправлены — история остаётся в старой панели.",
    );
    legacy.skip(
        "contests",
        dump.count("contests") as u64,
        "конкурсов. В новой системе у конкурсов свои условия и призы — заведите нужные в «Модули → Конкурсы».",
    );
    legacy.skip(
        "roulette",
        (dump.count("roulette_spins") + dump.count("roulette_tickets")) as u64,
        "записей рулетки. Неиспользованные вращения выдайте вручную в «Модули → Рулетка».",
    );
}

pub fn map(dump: &Dump) -> Legacy {
    let settings = settings(dump);
    let (currency, mixed) = decide_currency(dump, &settings);
    let mut legacy = Legacy::new(Source::Stealthnet, currency);
    legacy.dumped_at = dump.dumped_at;
    let option_days = tariffs(dump, &mut legacy);
    clients(dump, &mut legacy, &mixed);
    subscriptions(dump, &mut legacy);
    payments(dump, &mut legacy, &option_days);
    promos(dump, &mut legacy);
    tickets(dump, &mut legacy);
    leftovers(dump, &mut legacy);
    withdrawals(dump, &mut legacy);
    gifts(dump, &mut legacy);
    admins(dump, &mut legacy);
    auto_rules(dump, &mut legacy);
    legacy.settings = settings.into_iter().collect();
    legacy
}
