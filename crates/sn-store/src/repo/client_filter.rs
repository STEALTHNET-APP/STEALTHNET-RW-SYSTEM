use super::client_search::segment_condition;
use super::clients::ClientSegment;
use super::subscriptions::{PAID_LIKE, effective_status};
use crate::error::{StoreError, StoreResult};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sn_domain::client::Locale;
use sn_domain::format;
use sn_domain::ids::{CategoryId, TariffId};
use sn_domain::money::{Currency, Money};
use sn_domain::subscription::EXPIRING_SOON_DAYS;
use sqlx::Postgres;
use sqlx::postgres::PgArguments;
use sqlx::query::{Query, QueryAs, QueryScalar};
use std::collections::HashMap;

pub const FILTER_DAYS_MAX: u16 = 365;
pub const FILTER_IDS_MAX: usize = 50;
pub const FILTER_UTM_MAX: usize = 256;
pub const FILTER_AMOUNT_MAX: i64 = 1_000_000_000;

#[derive(Debug, Clone)]
pub enum SqlArg {
    Int(i64),
    Ints(Vec<i64>),
    Text(String),
    Texts(Vec<String>),
    Time(DateTime<Utc>),
    Flag(bool),
}

#[derive(Debug, Clone, Default)]
pub struct SqlArgs {
    list: Vec<SqlArg>,
}

impl SqlArgs {
    pub fn with_clock(now: DateTime<Utc>) -> StoreResult<Self> {
        let soon = now
            .checked_add_signed(Duration::days(EXPIRING_SOON_DAYS))
            .ok_or_else(|| StoreError::Invalid("дата вне допустимого диапазона".into()))?;
        let mut args = SqlArgs::default();
        args.push(SqlArg::Time(now));
        args.push(SqlArg::Time(soon));
        Ok(args)
    }

    pub fn push(&mut self, arg: SqlArg) -> String {
        self.list.push(arg);
        format!("${}", self.list.len())
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn bind<'q>(
        &'q self,
        mut query: Query<'q, Postgres, PgArguments>,
    ) -> Query<'q, Postgres, PgArguments> {
        for arg in &self.list {
            query = match arg {
                SqlArg::Int(v) => query.bind(*v),
                SqlArg::Ints(v) => query.bind(v.as_slice()),
                SqlArg::Text(v) => query.bind(v.as_str()),
                SqlArg::Texts(v) => query.bind(v.as_slice()),
                SqlArg::Time(v) => query.bind(*v),
                SqlArg::Flag(v) => query.bind(*v),
            };
        }
        query
    }

    pub fn bind_as<'q, O>(
        &'q self,
        mut query: QueryAs<'q, Postgres, O, PgArguments>,
    ) -> QueryAs<'q, Postgres, O, PgArguments> {
        for arg in &self.list {
            query = match arg {
                SqlArg::Int(v) => query.bind(*v),
                SqlArg::Ints(v) => query.bind(v.as_slice()),
                SqlArg::Text(v) => query.bind(v.as_str()),
                SqlArg::Texts(v) => query.bind(v.as_slice()),
                SqlArg::Time(v) => query.bind(*v),
                SqlArg::Flag(v) => query.bind(*v),
            };
        }
        query
    }

    pub fn bind_scalar<'q, O>(
        &'q self,
        mut query: QueryScalar<'q, Postgres, O, PgArguments>,
    ) -> QueryScalar<'q, Postgres, O, PgArguments> {
        for arg in &self.list {
            query = match arg {
                SqlArg::Int(v) => query.bind(*v),
                SqlArg::Ints(v) => query.bind(v.as_slice()),
                SqlArg::Text(v) => query.bind(v.as_str()),
                SqlArg::Texts(v) => query.bind(v.as_slice()),
                SqlArg::Time(v) => query.bind(*v),
                SqlArg::Flag(v) => query.bind(*v),
            };
        }
        query
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientOrigin {
    Bot,
    Miniapp,
    Web,
    Admin,
    Transfer,
}

impl ClientOrigin {
    pub const ALL: [ClientOrigin; 5] = [
        ClientOrigin::Bot,
        ClientOrigin::Miniapp,
        ClientOrigin::Web,
        ClientOrigin::Admin,
        ClientOrigin::Transfer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ClientOrigin::Bot => "bot",
            ClientOrigin::Miniapp => "miniapp",
            ClientOrigin::Web => "web",
            ClientOrigin::Admin => "admin",
            ClientOrigin::Transfer => "transfer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        ClientOrigin::ALL.into_iter().find(|o| o.as_str() == value)
    }

    pub const fn title(self) -> &'static str {
        match self {
            ClientOrigin::Bot => "бот",
            ClientOrigin::Miniapp => "мини-приложение",
            ClientOrigin::Web => "сайт",
            ClientOrigin::Admin => "админка",
            ClientOrigin::Transfer => "перенос",
        }
    }

    const fn sources(self) -> &'static [&'static str] {
        match self {
            ClientOrigin::Bot => &["bot"],
            ClientOrigin::Miniapp => &["miniapp"],
            ClientOrigin::Web => &["web"],
            ClientOrigin::Admin => &["admin"],
            ClientOrigin::Transfer => &["import", "migration"],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConditions {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tariff: Vec<TariffId>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub category: Vec<CategoryId>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "segment")]
    pub status: Option<ClientSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autorenew: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<ClientOrigin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<Locale>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_from: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_to: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_stopped: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invites: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utm: Option<String>,
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "yes" | "on" | "true" | "1" => Ok(true),
        "no" | "off" | "false" | "0" => Ok(false),
        _ => Err(format!("{key}: ожидается yes или no")),
    }
}

fn ids(key: &str, value: &str) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let id: i64 = part
            .parse()
            .map_err(|_| format!("{key}: ожидаются номера через запятую"))?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn days(key: &str, value: &str) -> Result<u16, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{key}: ожидается число дней"))
}

fn amount(key: &str, value: &str) -> Result<i64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{key}: ожидается сумма целым числом"))
}

fn instant(key: &str, value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|at| at.with_timezone(&Utc))
        .map_err(|_| format!("{key}: ожидается дата и время в формате RFC 3339"))
}

fn utm_ok(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl ClientConditions {
    pub fn from_query<I, K, V>(pairs: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut out = ClientConditions::default();
        for (key, value) in pairs {
            let key = key.as_ref();
            let value = value.as_ref().trim();
            if value.is_empty() {
                continue;
            }
            match key {
                "tariff" => out.tariff = ids(key, value)?.into_iter().map(TariffId).collect(),
                "category" => {
                    out.category = ids(key, value)?.into_iter().map(CategoryId).collect();
                }
                "status" | "segment" => {
                    if value != "all" {
                        out.status = Some(
                            ClientSegment::parse(value)
                                .map_err(|_| format!("неизвестный сегмент {value}"))?,
                        );
                    }
                }
                "ends" => out.ends = Some(days(key, value)?),
                "ended" => out.ended = Some(days(key, value)?),
                "autorenew" => out.autorenew = Some(flag(key, value)?),
                "source" => {
                    let mut list = Vec::new();
                    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                        let origin = ClientOrigin::parse(part)
                            .ok_or_else(|| format!("source: неизвестный источник {part}"))?;
                        if !list.contains(&origin) {
                            list.push(origin);
                        }
                    }
                    out.source = list;
                }
                "lang" => {
                    out.lang = Some(
                        Locale::parse(value)
                            .map_err(|_| format!("lang: неизвестный язык {value}"))?,
                    );
                }
                "from" => out.from = Some(instant(key, value)?),
                "to" => out.to = Some(instant(key, value)?),
                "paid" => out.paid = Some(flag(key, value)?),
                "paid_from" => out.paid_from = Some(amount(key, value)?),
                "paid_to" => out.paid_to = Some(amount(key, value)?),
                "balance" => out.balance = Some(flag(key, value)?),
                "telegram" => out.telegram = Some(flag(key, value)?),
                "email" => out.email = Some(flag(key, value)?),
                "bot_stopped" => out.bot_stopped = Some(flag(key, value)?),
                "invited" => out.invited = Some(flag(key, value)?),
                "invites" => out.invites = Some(flag(key, value)?),
                "utm" => out.utm = Some(value.to_string()),
                _ => {}
            }
        }
        out.check()?;
        Ok(out)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.tariff.len() > FILTER_IDS_MAX || self.category.len() > FILTER_IDS_MAX {
            return Err(format!(
                "Выберите не больше {FILTER_IDS_MAX} тарифов и категорий"
            ));
        }
        if self.tariff.iter().any(|t| t.0 <= 0) || self.category.iter().any(|c| c.0 <= 0) {
            return Err("Тариф или категория указаны неверно".into());
        }
        for value in [self.ends, self.ended].into_iter().flatten() {
            if value == 0 || value > FILTER_DAYS_MAX {
                return Err(format!("Число дней — от 1 до {FILTER_DAYS_MAX}"));
            }
        }
        if let (Some(from), Some(to)) = (self.from, self.to)
            && from >= to
        {
            return Err("Дата регистрации: начало должно быть раньше конца".into());
        }
        for value in [self.paid_from, self.paid_to].into_iter().flatten() {
            if !(0..=FILTER_AMOUNT_MAX).contains(&value) {
                return Err("Сумма оплат указана неверно".into());
            }
        }
        if let (Some(low), Some(high)) = (self.paid_from, self.paid_to)
            && low > high
        {
            return Err("Сумма оплат: «от» больше, чем «до»".into());
        }
        if let Some(utm) = &self.utm
            && (utm.is_empty() || utm.len() > FILTER_UTM_MAX || !utm_ok(utm))
        {
            return Err(
                "Метка: латинские буквы, цифры, дефис и подчёркивание, до 256 знаков".into(),
            );
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        *self == ClientConditions::default()
    }

    pub fn uses_money(&self) -> bool {
        self.paid.is_some()
            || self.paid_from.is_some()
            || self.paid_to.is_some()
            || self.balance.is_some()
    }

    pub fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let join = |list: Vec<String>| list.join(",");
        let yes = |value: bool| if value { "yes" } else { "no" }.to_string();
        let mut out = Vec::new();
        if !self.tariff.is_empty() {
            out.push((
                "tariff",
                join(self.tariff.iter().map(|t| t.0.to_string()).collect()),
            ));
        }
        if !self.category.is_empty() {
            out.push((
                "category",
                join(self.category.iter().map(|c| c.0.to_string()).collect()),
            ));
        }
        if let Some(status) = self.status {
            out.push(("status", status.as_str().to_string()));
        }
        if let Some(value) = self.ends {
            out.push(("ends", value.to_string()));
        }
        if let Some(value) = self.ended {
            out.push(("ended", value.to_string()));
        }
        if let Some(value) = self.autorenew {
            out.push(("autorenew", yes(value)));
        }
        if !self.source.is_empty() {
            out.push((
                "source",
                join(self.source.iter().map(|o| o.as_str().to_string()).collect()),
            ));
        }
        if let Some(lang) = self.lang {
            out.push(("lang", lang.as_str().to_string()));
        }
        if let Some(at) = self.from {
            out.push(("from", at.to_rfc3339()));
        }
        if let Some(at) = self.to {
            out.push(("to", at.to_rfc3339()));
        }
        for (key, value) in [
            ("paid", self.paid),
            ("balance", self.balance),
            ("telegram", self.telegram),
            ("email", self.email),
            ("bot_stopped", self.bot_stopped),
            ("invited", self.invited),
            ("invites", self.invites),
        ] {
            if let Some(value) = value {
                out.push((key, yes(value)));
            }
        }
        if let Some(value) = self.paid_from {
            out.push(("paid_from", value.to_string()));
        }
        if let Some(value) = self.paid_to {
            out.push(("paid_to", value.to_string()));
        }
        if let Some(utm) = &self.utm {
            out.push(("utm", utm.clone()));
        }
        out
    }

    pub fn conditions(
        &self,
        args: &mut SqlArgs,
        now: DateTime<Utc>,
        currency: Currency,
    ) -> StoreResult<Vec<String>> {
        let shift = |n: u16, forward: bool| {
            let span = Duration::days(i64::from(n));
            if forward {
                now.checked_add_signed(span)
            } else {
                now.checked_sub_signed(span)
            }
            .ok_or_else(|| StoreError::Invalid("дата вне допустимого диапазона".into()))
        };
        let est = effective_status("s", "$1");
        let mut out = Vec::new();
        if !self.tariff.is_empty() {
            let list = args.push(SqlArg::Ints(self.tariff.iter().map(|t| t.0).collect()));
            out.push(format!(
                "c.id IN (SELECT ft.client_id FROM subscriptions ft WHERE ft.ended_at IS NULL \
                 AND ft.tariff_id = ANY({list}))"
            ));
        }
        if !self.category.is_empty() {
            let list = args.push(SqlArg::Ints(self.category.iter().map(|c| c.0).collect()));
            out.push(format!(
                "c.id IN (SELECT fc.client_id FROM subscriptions fc JOIN tariffs fct ON fct.id = fc.tariff_id \
                 WHERE fc.ended_at IS NULL AND fct.category_id = ANY({list}))"
            ));
        }
        if let Some(segment) = self.status {
            out.push(segment_condition(segment));
        }
        if let Some(value) = self.ends {
            let until = args.push(SqlArg::Time(shift(value, true)?));
            out.push(format!(
                "{est} = 'active' AND s.expire_at > $1 AND s.expire_at <= {until} \
                 AND c.id IN (SELECT fe.client_id FROM subscriptions fe WHERE fe.ended_at IS NULL \
                 AND fe.remna_missing_at IS NULL AND fe.expire_at > $1 AND fe.expire_at <= {until})"
            ));
        }
        if let Some(value) = self.ended {
            let since = args.push(SqlArg::Time(shift(value, false)?));
            out.push(format!(
                "{est} = 'expired' AND s.expire_at <= $1 AND s.expire_at > {since} \
                 AND c.id IN (SELECT fx.client_id FROM subscriptions fx WHERE fx.ended_at IS NULL \
                 AND fx.remna_missing_at IS NULL AND fx.expire_at <= $1 AND fx.expire_at > {since})"
            ));
        }
        match self.autorenew {
            Some(true) => out.push(
                "s.autorenew_enabled AND c.id IN (SELECT fa.client_id FROM subscriptions fa \
                 WHERE fa.ended_at IS NULL AND fa.autorenew_enabled)"
                    .into(),
            ),
            Some(false) => out.push("s.id IS NOT NULL AND NOT s.autorenew_enabled".into()),
            None => {}
        }
        if !self.source.is_empty() {
            let list = args.push(SqlArg::Texts(
                self.source
                    .iter()
                    .flat_map(|o| o.sources().iter().map(|s| (*s).to_string()))
                    .collect(),
            ));
            out.push(format!("c.registration_source = ANY({list})"));
        }
        if let Some(lang) = self.lang {
            let value = args.push(SqlArg::Text(lang.as_str().into()));
            out.push(format!("c.locale = {value}"));
        }
        if let Some(at) = self.from {
            let value = args.push(SqlArg::Time(at));
            out.push(format!("c.created_at >= {value}"));
        }
        if let Some(at) = self.to {
            let value = args.push(SqlArg::Time(at));
            out.push(format!("c.created_at < {value}"));
        }
        let paid_rows = format!(
            "SELECT fp.client_id FROM payments fp WHERE fp.status IN {PAID_LIKE} AND fp.provider <> 'balance'"
        );
        match self.paid {
            Some(true) => out.push(format!("c.id IN ({paid_rows})")),
            Some(false) => out.push(format!(
                "NOT EXISTS (SELECT 1 FROM payments fp WHERE fp.client_id = c.id \
                 AND fp.status IN {PAID_LIKE} AND fp.provider <> 'balance')"
            )),
            None => {}
        }
        let low = self.paid_from.filter(|v| *v > 0);
        if low.is_some() || self.paid_to.is_some() {
            let code = args.push(SqlArg::Text(currency.code().into()));
            let scale = currency.minor_per_unit();
            let totals = format!(
                "SELECT fs.client_id FROM payments fs WHERE fs.currency = {code} \
                 AND fs.status IN {PAID_LIKE} AND fs.provider <> 'balance' GROUP BY fs.client_id"
            );
            let sum = "sum(fs.amount_minor - fs.refunded_minor)";
            match (low, self.paid_to) {
                (Some(low), high) => {
                    let from = args.push(SqlArg::Int(low.saturating_mul(scale)));
                    let upper = high
                        .map(|high| {
                            let to = args.push(SqlArg::Int(high.saturating_mul(scale)));
                            format!(" AND {sum} <= {to}")
                        })
                        .unwrap_or_default();
                    out.push(format!("c.id IN ({totals} HAVING {sum} >= {from}{upper})"));
                }
                (None, Some(high)) => {
                    let to = args.push(SqlArg::Int(high.saturating_mul(scale)));
                    out.push(format!(
                        "NOT EXISTS (SELECT 1 FROM ({totals} HAVING {sum} > {to}) fx \
                         WHERE fx.client_id = c.id)"
                    ));
                }
                (None, None) => {}
            }
        }
        if let Some(value) = self.balance {
            let code = args.push(SqlArg::Text(currency.code().into()));
            out.push(if value {
                format!(
                    "c.id IN (SELECT fb.client_id FROM client_balances fb WHERE fb.wallet = 'main' \
                     AND fb.currency = {code} AND fb.amount_minor > 0)"
                )
            } else {
                format!(
                    "NOT EXISTS (SELECT 1 FROM client_balances fb WHERE fb.client_id = c.id \
                     AND fb.wallet = 'main' AND fb.currency = {code} AND fb.amount_minor > 0)"
                )
            });
        }
        for (kind, value) in [("telegram", self.telegram), ("email", self.email)] {
            match value {
                Some(true) => out.push(format!(
                    "c.id IN (SELECT fi.client_id FROM client_identities fi WHERE fi.kind = '{kind}')"
                )),
                Some(false) => out.push(format!(
                    "NOT EXISTS (SELECT 1 FROM client_identities fi WHERE fi.client_id = c.id \
                     AND fi.kind = '{kind}')"
                )),
                None => {}
            }
        }
        if let Some(value) = self.bot_stopped {
            out.push(if value {
                "c.bot_unreachable".into()
            } else {
                "NOT c.bot_unreachable".into()
            });
        }
        match self.invited {
            Some(true) => out.push("c.id IN (SELECT fr.client_id FROM referral_links fr)".into()),
            Some(false) => out.push(
                "NOT EXISTS (SELECT 1 FROM referral_links fr WHERE fr.client_id = c.id)".into(),
            ),
            None => {}
        }
        match self.invites {
            Some(true) => out.push("c.id IN (SELECT fr.referrer_id FROM referral_links fr)".into()),
            Some(false) => out.push(
                "NOT EXISTS (SELECT 1 FROM referral_links fr WHERE fr.referrer_id = c.id)".into(),
            ),
            None => {}
        }
        if let Some(utm) = &self.utm {
            let value = args.push(SqlArg::Text(utm.to_lowercase()));
            out.push(format!(
                "c.id IN (SELECT fu.client_id FROM client_utm fu WHERE {value} IN (lower(fu.source), \
                 lower(fu.medium), lower(fu.campaign), lower(fu.content), lower(fu.term), lower(fu.start_param)))"
            ));
        }
        Ok(out)
    }

    pub fn describe(
        &self,
        tariffs: &HashMap<TariffId, String>,
        categories: &HashMap<CategoryId, String>,
        currency: Currency,
    ) -> Vec<String> {
        let names = |list: Vec<String>| {
            list.iter()
                .map(|n| format!("«{n}»"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut out = Vec::new();
        if !self.tariff.is_empty() {
            let list: Vec<String> = self
                .tariff
                .iter()
                .map(|t| {
                    tariffs
                        .get(t)
                        .cloned()
                        .unwrap_or_else(|| format!("№{}", t.0))
                })
                .collect();
            out.push(format!(
                "{} {}",
                if list.len() == 1 {
                    "тариф"
                } else {
                    "тарифы"
                },
                names(list)
            ));
        }
        if !self.category.is_empty() {
            let list: Vec<String> = self
                .category
                .iter()
                .map(|c| {
                    categories
                        .get(c)
                        .cloned()
                        .unwrap_or_else(|| format!("№{}", c.0))
                })
                .collect();
            out.push(format!(
                "{} {}",
                if list.len() == 1 {
                    "категория"
                } else {
                    "категории"
                },
                names(list)
            ));
        }
        if let Some(status) = self.status {
            out.push(status.title().to_string());
        }
        if let Some(n) = self.ends {
            out.push(format!(
                "подписка закончится в ближайшие {}",
                format::days(u32::from(n))
            ));
        }
        if let Some(n) = self.ended {
            out.push(format!(
                "подписка закончилась за последние {}",
                format::days(u32::from(n))
            ));
        }
        if let Some(value) = self.autorenew {
            out.push(if value {
                "автопродление включено".into()
            } else {
                "автопродление выключено".into()
            });
        }
        if !self.source.is_empty() {
            out.push(format!(
                "откуда: {}",
                self.source
                    .iter()
                    .map(|o| o.title())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(lang) = self.lang {
            out.push(match lang {
                Locale::Ru => "язык русский".into(),
                Locale::En => "язык английский".into(),
            });
        }
        match (self.from, self.to) {
            (Some(from), Some(to)) => out.push(format!(
                "регистрация с {} по {}",
                from.format("%d.%m.%Y"),
                (to - Duration::seconds(1)).format("%d.%m.%Y")
            )),
            (Some(from), None) => out.push(format!("регистрация с {}", from.format("%d.%m.%Y"))),
            (None, Some(to)) => out.push(format!(
                "регистрация по {}",
                (to - Duration::seconds(1)).format("%d.%m.%Y")
            )),
            (None, None) => {}
        }
        if let Some(value) = self.paid {
            out.push(
                if value {
                    "платили"
                } else {
                    "не платили"
                }
                .into(),
            );
        }
        let units = |value: i64| {
            format::money_compact(Money::new(
                value.saturating_mul(currency.minor_per_unit()),
                currency,
            ))
        };
        match (self.paid_from, self.paid_to) {
            (Some(low), Some(high)) => {
                out.push(format!("оплатили от {} до {}", units(low), units(high)));
            }
            (Some(low), None) => out.push(format!("оплатили от {}", units(low))),
            (None, Some(high)) => out.push(format!("оплатили до {}", units(high))),
            (None, None) => {}
        }
        if let Some(value) = self.balance {
            out.push(
                if value {
                    "есть деньги на балансе"
                } else {
                    "баланс пустой"
                }
                .into(),
            );
        }
        if let Some(value) = self.telegram {
            out.push(
                if value {
                    "есть Telegram"
                } else {
                    "без Telegram"
                }
                .into(),
            );
        }
        if let Some(value) = self.email {
            out.push(
                if value {
                    "есть почта"
                } else {
                    "без почты"
                }
                .into(),
            );
        }
        if let Some(value) = self.bot_stopped {
            out.push(
                if value {
                    "бот остановлен"
                } else {
                    "бот доступен"
                }
                .into(),
            );
        }
        if let Some(value) = self.invited {
            out.push(
                if value {
                    "пришли по приглашению"
                } else {
                    "пришли не по приглашению"
                }
                .into(),
            );
        }
        if let Some(value) = self.invites {
            out.push(
                if value {
                    "приглашали друзей"
                } else {
                    "не приглашали друзей"
                }
                .into(),
            );
        }
        if let Some(utm) = &self.utm {
            out.push(format!("метка {utm}"));
        }
        out
    }
}

impl ClientConditions {
    pub fn needs_primary(&self) -> bool {
        self.status.is_some()
            || self.ends.is_some()
            || self.ended.is_some()
            || self.autorenew.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilterTariff {
    pub id: TariffId,
    pub name: String,
    pub category_id: CategoryId,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilterCategory {
    pub id: CategoryId,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FilterOptions {
    pub tariffs: Vec<FilterTariff>,
    pub categories: Vec<FilterCategory>,
    pub utm: Vec<String>,
}

pub const FILTER_UTM_SUGGESTIONS: i64 = 40;

impl FilterOptions {
    pub fn tariff_names(&self) -> HashMap<TariffId, String> {
        self.tariffs
            .iter()
            .map(|t| (t.id, t.name.clone()))
            .collect()
    }

    pub fn category_names(&self) -> HashMap<CategoryId, String> {
        self.categories
            .iter()
            .map(|c| (c.id, c.name.clone()))
            .collect()
    }
}

pub async fn filter_options(conn: &mut sqlx::PgConnection) -> StoreResult<FilterOptions> {
    let tariffs: Vec<(i64, String, i64, bool)> = sqlx::query_as(
        "SELECT t.id, t.name, t.category_id, t.deleted_at IS NOT NULL FROM tariffs t \
         JOIN tariff_categories k ON k.id = t.category_id \
         WHERE t.deleted_at IS NULL OR EXISTS (SELECT 1 FROM subscriptions s \
             WHERE s.tariff_id = t.id AND s.ended_at IS NULL) \
         ORDER BY k.deleted_at IS NOT NULL, k.sort, k.id, t.deleted_at IS NOT NULL, t.sort, t.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let categories: Vec<(i64, String)> = sqlx::query_as(
        "SELECT k.id, k.name FROM tariff_categories k \
         WHERE k.deleted_at IS NULL OR EXISTS (SELECT 1 FROM tariffs t JOIN subscriptions s ON s.tariff_id = t.id \
             WHERE t.category_id = k.id AND s.ended_at IS NULL) \
         ORDER BY k.deleted_at IS NOT NULL, k.sort, k.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let utm: Vec<String> = sqlx::query_scalar(
        "SELECT v FROM client_utm u CROSS JOIN LATERAL (VALUES (u.source), (u.campaign)) x(v) \
         WHERE v IS NOT NULL GROUP BY v ORDER BY count(*) DESC, v LIMIT $1",
    )
    .bind(FILTER_UTM_SUGGESTIONS)
    .fetch_all(&mut *conn)
    .await?;
    Ok(FilterOptions {
        tariffs: tariffs
            .into_iter()
            .map(|(id, name, category_id, deleted)| FilterTariff {
                id: TariffId(id),
                name,
                category_id: CategoryId(category_id),
                deleted,
            })
            .collect(),
        categories: categories
            .into_iter()
            .map(|(id, name)| FilterCategory {
                id: CategoryId(id),
                name,
            })
            .collect(),
        utm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(query: &str) -> Result<ClientConditions, String> {
        ClientConditions::from_query(
            query
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| p.split_once('=').unwrap_or((p, ""))),
        )
    }

    #[test]
    fn условия_разбираются_из_адреса() {
        assert_eq!(parse("").unwrap(), ClientConditions::default());
        assert_eq!(
            parse("q=иван&limit=5&after=7&status=all").unwrap(),
            ClientConditions::default()
        );
        let c = parse("tariff=3,5,3&category=2&segment=expired&ends=7&autorenew=no&source=bot,transfer,bot&lang=en&paid=yes&paid_from=10&paid_to=500&balance=no&telegram=yes&email=no&bot_stopped=yes&invited=yes&invites=no&utm=tiktok").unwrap();
        assert_eq!(c.tariff, vec![TariffId(3), TariffId(5)]);
        assert_eq!(c.category, vec![CategoryId(2)]);
        assert_eq!(c.status, Some(ClientSegment::Expired));
        assert_eq!(c.ends, Some(7));
        assert_eq!(c.autorenew, Some(false));
        assert_eq!(c.source, vec![ClientOrigin::Bot, ClientOrigin::Transfer]);
        assert_eq!(c.lang, Some(Locale::En));
        assert_eq!(c.paid, Some(true));
        assert_eq!((c.paid_from, c.paid_to), (Some(10), Some(500)));
        assert_eq!(c.balance, Some(false));
        assert_eq!((c.telegram, c.email), (Some(true), Some(false)));
        assert_eq!(c.bot_stopped, Some(true));
        assert_eq!((c.invited, c.invites), (Some(true), Some(false)));
        assert_eq!(c.utm.as_deref(), Some("tiktok"));
        assert!(c.uses_money());
        assert!(c.needs_primary());
        assert_eq!(
            parse("status=active").unwrap().status,
            Some(ClientSegment::Active)
        );
        let at = parse("from=2026-11-01T00:00:00+03:00&to=2026-11-02T00:00:00Z").unwrap();
        assert_eq!(
            at.from,
            Some(
                DateTime::parse_from_rfc3339("2026-10-31T21:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        assert!(!at.uses_money());
        assert!(!at.needs_primary());
    }

    #[test]
    fn неверные_условия_отклоняются() {
        for bad in [
            "tariff=x",
            "tariff=0",
            "category=-2",
            "segment=lost",
            "ends=0",
            "ends=366",
            "ended=abc",
            "autorenew=maybe",
            "source=ufo",
            "lang=de",
            "from=2026-11-01",
            "from=2026-11-02T00:00:00Z&to=2026-11-01T00:00:00Z",
            "paid_from=-1",
            "paid_from=10&paid_to=5",
            "paid_to=1000000001",
            "utm=a%20b",
            "utm=ab;cd",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let many = (1..=51)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse(&format!("tariff={many}")).is_err());
        assert!(parse(&format!("utm={}", "a".repeat(257))).is_err());
        assert!(parse(&format!("utm={}", "a".repeat(256))).is_ok());
    }

    #[test]
    fn условия_сохраняются_и_читаются_без_потерь() {
        let c = parse("tariff=1,3&status=trial&ended=30&autorenew=yes&source=web&lang=ru&from=2026-11-01T00:00:00Z&to=2026-12-01T00:00:00Z&paid=no&balance=yes&telegram=no&email=yes&bot_stopped=no&invited=no&invites=yes&utm=s_vk__k_spring").unwrap();
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["tariff"], serde_json::json!([1, 3]));
        assert_eq!(json["status"], "trial");
        assert!(json.get("category").is_none());
        assert_eq!(serde_json::from_value::<ClientConditions>(json).unwrap(), c);
        assert_eq!(ClientConditions::from_query(c.query_pairs()).unwrap(), c);
        assert_eq!(
            serde_json::to_value(ClientConditions::default()).unwrap(),
            serde_json::json!({})
        );
        let old: ClientConditions =
            serde_json::from_value(serde_json::json!({"segment": "limited", "unknown": 1}))
                .unwrap();
        assert_eq!(old.status, Some(ClientSegment::Limited));
    }

    #[test]
    fn условия_описываются_словами() {
        let c = parse("tariff=1,2&category=9&status=active&ends=7&source=bot,transfer&lang=en&from=2026-11-01T00:00:00Z&to=2026-11-08T00:00:00Z&paid_from=10&paid_to=500&balance=yes&telegram=yes&email=no&bot_stopped=no&invited=yes&invites=no&utm=tiktok").unwrap();
        let tariffs = HashMap::from([(TariffId(1), "Стандарт".to_string())]);
        let categories = HashMap::from([(CategoryId(9), "Unblock".to_string())]);
        let text = c.describe(&tariffs, &categories, Currency::Rub);
        assert_eq!(
            text,
            vec![
                "тарифы «Стандарт», «№2»",
                "категория «Unblock»",
                "подписка действует",
                "подписка закончится в ближайшие 7\u{a0}дней",
                "откуда: бот, перенос",
                "язык английский",
                "регистрация с 01.11.2026 по 07.11.2026",
                "оплатили от 10\u{a0}₽ до 500\u{a0}₽",
                "есть деньги на балансе",
                "есть Telegram",
                "без почты",
                "бот доступен",
                "пришли по приглашению",
                "не приглашали друзей",
                "метка tiktok",
            ]
        );
        assert!(
            ClientConditions::default()
                .describe(&tariffs, &categories, Currency::Usd)
                .is_empty()
        );
    }
}
