use chrono::{DateTime, Datelike, Days, FixedOffset, Months, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Postgres;
use sqlx::postgres::PgArguments;
use sqlx::query::QueryAs;

pub const MAX_POINTS: usize = 120;
pub const MAX_DAYS: i64 = 3_700;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Compare {
    #[default]
    Prev,
    Year,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Dates {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl Dates {
    pub fn days(&self) -> i64 {
        (self.to - self.from).num_days() + 1
    }

    pub fn contains(&self, day: NaiveDate) -> bool {
        self.from <= day && day <= self.to
    }
}

#[derive(Debug, Clone)]
pub struct Range {
    pub offset: FixedOffset,
    pub dates: Dates,
    pub step: Step,
    pub prev: Option<Dates>,
    pub today: NaiveDate,
    pub now: DateTime<Utc>,
}

pub fn midnight(day: NaiveDate, offset: FixedOffset) -> DateTime<Utc> {
    let local = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    offset
        .from_local_datetime(&local)
        .single()
        .map_or_else(|| local.and_utc(), |t| t.with_timezone(&Utc))
}

pub fn local_day(at: DateTime<Utc>, offset: FixedOffset) -> NaiveDate {
    at.with_timezone(&offset).date_naive()
}

fn next_monday(day: NaiveDate) -> NaiveDate {
    let shift = 7 - u64::from(day.weekday().num_days_from_monday());
    day.checked_add_days(Days::new(shift)).unwrap_or(day)
}

fn next_month(day: NaiveDate) -> NaiveDate {
    let first = day.with_day(1).unwrap_or(day);
    first.checked_add_months(Months::new(1)).unwrap_or(day)
}

pub fn starts(dates: Dates, step: Step) -> Vec<NaiveDate> {
    let mut out = vec![dates.from];
    let mut day = dates.from;
    loop {
        day = match step {
            Step::Day => day.checked_add_days(Days::new(1)).unwrap_or(day),
            Step::Week => next_monday(day),
            Step::Month => next_month(day),
        };
        if day > dates.to || out.len() > MAX_DAYS as usize {
            break;
        }
        out.push(day);
    }
    out
}

pub fn fit_step(dates: Dates, wanted: Option<Step>) -> Step {
    let auto = if dates.days() <= 45 {
        Step::Day
    } else if dates.days() <= 190 {
        Step::Week
    } else {
        Step::Month
    };
    let mut step = wanted.unwrap_or(auto);
    loop {
        if starts(dates, step).len() <= MAX_POINTS || step == Step::Month {
            return step;
        }
        step = match step {
            Step::Day => Step::Week,
            _ => Step::Month,
        };
    }
}

fn month_number(day: NaiveDate) -> u32 {
    u32::try_from(day.year()).unwrap_or(0) * 12 + day.month0()
}

fn month_end(day: NaiveDate) -> Option<NaiveDate> {
    day.with_day(1)?
        .checked_add_months(Months::new(1))?
        .pred_opt()
}

pub fn previous(dates: Dates, compare: Compare, rolling: bool) -> Option<Dates> {
    match compare {
        Compare::None => None,
        Compare::Prev => {
            if !rolling && dates.from.day() == 1 && dates.days() > 1 {
                let months = month_number(dates.to) - month_number(dates.from) + 1;
                if month_end(dates.to) == Some(dates.to) {
                    return Some(Dates {
                        from: dates.from.checked_sub_months(Months::new(months))?,
                        to: dates.from.pred_opt()?,
                    });
                }
                return Some(Dates {
                    from: dates.from.checked_sub_months(Months::new(months))?,
                    to: dates.to.checked_sub_months(Months::new(months))?,
                });
            }
            let len = Days::new(u64::try_from(dates.days()).unwrap_or(1));
            let to = dates.from.checked_sub_days(Days::new(1))?;
            let from = dates.from.checked_sub_days(len)?;
            Some(Dates { from, to })
        }
        Compare::Year => Some(Dates {
            from: dates.from.checked_sub_months(Months::new(12))?,
            to: dates.to.checked_sub_months(Months::new(12))?,
        }),
    }
}

impl Range {
    pub fn start(&self) -> DateTime<Utc> {
        midnight(self.dates.from, self.offset)
    }

    pub fn end(&self) -> DateTime<Utc> {
        bound_after(self.dates.to, self.offset)
    }

    pub fn bounds(&self) -> Vec<DateTime<Utc>> {
        bounds_of(self.dates, self.step, self.offset)
    }

    pub fn labels(&self) -> Vec<NaiveDate> {
        starts(self.dates, self.step)
    }

    pub fn prev_bounds(&self) -> Option<Vec<DateTime<Utc>>> {
        let (_, cap) = self.prev_window()?;
        self.prev.map(|p| {
            bounds_of(p, self.step, self.offset)
                .into_iter()
                .map(|b| b.min(cap))
                .collect()
        })
    }

    pub fn prev_labels(&self) -> Option<Vec<NaiveDate>> {
        self.prev.map(|p| starts(p, self.step))
    }

    pub fn live(&self) -> bool {
        self.dates.contains(self.today)
    }

    pub fn moment(&self) -> DateTime<Utc> {
        self.end().min(self.now)
    }

    pub fn prev_window(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        self.prev.map(|p| {
            let start = midnight(p.from, self.offset);
            let end = bound_after(p.to, self.offset);
            if !self.live() {
                return (start, end);
            }
            let left = (self.end() - self.now).max(chrono::Duration::zero());
            (start, (end - left).max(start))
        })
    }
}

pub fn bound_after(day: NaiveDate, offset: FixedOffset) -> DateTime<Utc> {
    midnight(day.checked_add_days(Days::new(1)).unwrap_or(day), offset)
}

pub fn bounds_of(dates: Dates, step: Step, offset: FixedOffset) -> Vec<DateTime<Utc>> {
    let mut out: Vec<DateTime<Utc>> = starts(dates, step)
        .into_iter()
        .map(|d| midnight(d, offset))
        .collect();
    out.push(bound_after(dates.to, offset));
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceFilter {
    Utm {
        source: String,
        campaign: Option<String>,
    },
    Promo(String),
    Referral,
    Gift,
    Unlabeled,
}

impl SourceFilter {
    pub fn parse(text: &str) -> Option<SourceFilter> {
        let text = text.trim();
        match text {
            "" => None,
            "ref" => Some(SourceFilter::Referral),
            "gift" => Some(SourceFilter::Gift),
            "none" => Some(SourceFilter::Unlabeled),
            _ => {
                if let Some(rest) = text.strip_prefix("utm:") {
                    let (source, campaign) = match rest.split_once('/') {
                        Some((s, c)) => (s, Some(c.to_string()).filter(|c| !c.is_empty())),
                        None => (rest, None),
                    };
                    return Some(SourceFilter::Utm {
                        source: source.to_string(),
                        campaign,
                    });
                }
                text.strip_prefix("promo:")
                    .map(|id| SourceFilter::Promo(id.to_string()))
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub tariff: Option<i64>,
    pub category: Option<i64>,
    pub channel: Option<String>,
    pub source: Option<SourceFilter>,
    pub provider: Option<String>,
    pub method: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Tariff,
    Category,
    Channel,
    Source,
    Provider,
    Method,
}

impl Dim {
    const fn title(self) -> &'static str {
        match self {
            Dim::Tariff => "тарифу",
            Dim::Category => "категории",
            Dim::Channel => "каналу",
            Dim::Source => "источнику",
            Dim::Provider => "платёжке",
            Dim::Method => "способу оплаты",
        }
    }
}

pub const PAYMENT_DIMS: [Dim; 6] = [
    Dim::Tariff,
    Dim::Category,
    Dim::Channel,
    Dim::Source,
    Dim::Provider,
    Dim::Method,
];
pub const SUBSCRIPTION_DIMS: [Dim; 3] = [Dim::Tariff, Dim::Category, Dim::Source];
pub const CLIENT_DIMS: [Dim; 1] = [Dim::Source];
pub const NO_DIMS: [Dim; 0] = [];

impl Filters {
    pub fn active(&self) -> Vec<Dim> {
        let mut out = Vec::new();
        if self.tariff.is_some() {
            out.push(Dim::Tariff);
        }
        if self.category.is_some() {
            out.push(Dim::Category);
        }
        if self.channel.is_some() {
            out.push(Dim::Channel);
        }
        if self.source.is_some() {
            out.push(Dim::Source);
        }
        if self.provider.is_some() {
            out.push(Dim::Provider);
        }
        if self.method.is_some() {
            out.push(Dim::Method);
        }
        out
    }

    pub fn ignored(&self, applies: &[Dim]) -> Option<String> {
        let skipped: Vec<&str> = self
            .active()
            .into_iter()
            .filter(|d| !applies.contains(d))
            .map(Dim::title)
            .collect();
        if skipped.is_empty() {
            None
        } else {
            Some(format!("без фильтра по {}", skipped.join(", ")))
        }
    }

    pub fn source_cond(&self, sql: &mut Sql, ac: &str) -> String {
        match &self.source {
            None => "true".into(),
            Some(SourceFilter::Utm { source, campaign }) => {
                let s = sql.arg(Arg::Text(source.clone()));
                let mut cond = format!("({ac}.source_type = 'utm' AND {ac}.source_key = {s}");
                if let Some(c) = campaign {
                    let c = sql.arg(Arg::Text(c.clone()));
                    cond.push_str(&format!(" AND {ac}.utm_campaign = {c}"));
                }
                cond.push(')');
                cond
            }
            Some(SourceFilter::Promo(id)) => {
                let k = sql.arg(Arg::Text(id.clone()));
                format!("({ac}.source_type = 'promo' AND {ac}.source_key = {k})")
            }
            Some(SourceFilter::Referral) => format!("{ac}.source_type = 'ref'"),
            Some(SourceFilter::Gift) => format!("{ac}.source_type = 'gift'"),
            Some(SourceFilter::Unlabeled) => format!("{ac}.source_type = 'none'"),
        }
    }

    pub fn client(&self, sql: &mut Sql, client_col: &str) -> String {
        if self.source.is_none() {
            return "true".into();
        }
        let cond = self.source_cond(sql, "fc");
        format!(
            "EXISTS (SELECT 1 FROM analytics_clients fc WHERE fc.client_id = {client_col} AND {cond})"
        )
    }

    pub fn tariff_cond(&self, sql: &mut Sql, tariff_col: &str) -> String {
        let mut parts = Vec::new();
        if let Some(t) = self.tariff {
            let p = sql.arg(Arg::Int(t));
            parts.push(format!("{tariff_col} = {p}"));
        }
        if let Some(c) = self.category {
            let p = sql.arg(Arg::Int(c));
            parts.push(format!(
                "{tariff_col} IN (SELECT ft.id FROM tariffs ft WHERE ft.category_id = {p})"
            ));
        }
        if parts.is_empty() {
            "true".into()
        } else {
            parts.join(" AND ")
        }
    }

    pub fn facts(&self, sql: &mut Sql, a: &str) -> String {
        let mut parts = vec![self.tariff_cond(sql, &format!("{a}.tariff_id"))];
        if let Some(ch) = &self.channel {
            let value = if ch == "unknown" {
                String::new()
            } else {
                ch.clone()
            };
            let p = sql.arg(Arg::Text(value));
            parts.push(format!("{a}.channel = {p}"));
        }
        if let Some(pr) = &self.provider {
            let p = sql.arg(Arg::Text(pr.clone()));
            parts.push(format!("{a}.provider = {p}"));
        }
        if let Some(m) = &self.method {
            let p = sql.arg(Arg::Text(m.clone()));
            parts.push(format!("{a}.method = {p}"));
        }
        parts.push(self.client(sql, &format!("{a}.client_id")));
        parts.join(" AND ")
    }

    pub fn spans(&self, sql: &mut Sql, a: &str) -> String {
        let tariff = self.tariff_cond(sql, &format!("{a}.tariff_id"));
        let client = self.client(sql, &format!("{a}.client_id"));
        format!("{tariff} AND {client}")
    }
}

#[derive(Debug, Clone)]
pub enum Arg {
    Int(i64),
    Text(String),
    Time(DateTime<Utc>),
    Times(Vec<DateTime<Utc>>),
    Ints(Vec<i64>),
    Texts(Vec<String>),
    Float(f64),
}

#[derive(Debug, Clone, Default)]
pub struct Sql {
    pub args: Vec<Arg>,
}

impl Sql {
    pub fn arg(&mut self, arg: Arg) -> String {
        self.args.push(arg);
        format!("${}", self.args.len())
    }

    pub fn time(&mut self, at: DateTime<Utc>) -> String {
        self.arg(Arg::Time(at))
    }

    pub fn times(&mut self, at: Vec<DateTime<Utc>>) -> String {
        format!("{}::timestamptz[]", self.arg(Arg::Times(at)))
    }

    pub fn text(&mut self, value: &str) -> String {
        self.arg(Arg::Text(value.to_string()))
    }

    pub fn int(&mut self, value: i64) -> String {
        self.arg(Arg::Int(value))
    }
}

pub type Q<'q, O> = QueryAs<'q, Postgres, O, PgArguments>;

pub fn bind<'q, O>(mut query: Q<'q, O>, args: &'q [Arg]) -> Q<'q, O> {
    for arg in args {
        query = match arg {
            Arg::Int(v) => query.bind(*v),
            Arg::Text(v) => query.bind(v.as_str()),
            Arg::Time(v) => query.bind(*v),
            Arg::Times(v) => query.bind(v.as_slice()),
            Arg::Ints(v) => query.bind(v.as_slice()),
            Arg::Texts(v) => query.bind(v.as_slice()),
            Arg::Float(v) => query.bind(*v),
        };
    }
    query
}

pub async fn rows<O>(
    db: &mut impl sn_store::Db,
    sql: &Sql,
    text: &str,
) -> sn_store::StoreResult<Vec<O>>
where
    O: Send + Unpin + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>,
{
    Ok(bind(sqlx::query_as(text), &sql.args)
        .fetch_all(db.conn())
        .await?)
}

pub fn bind_query<'q>(
    mut query: sqlx::query::Query<'q, Postgres, PgArguments>,
    args: &'q [Arg],
) -> sqlx::query::Query<'q, Postgres, PgArguments> {
    for arg in args {
        query = match arg {
            Arg::Int(v) => query.bind(*v),
            Arg::Text(v) => query.bind(v.as_str()),
            Arg::Time(v) => query.bind(*v),
            Arg::Times(v) => query.bind(v.as_slice()),
            Arg::Ints(v) => query.bind(v.as_slice()),
            Arg::Texts(v) => query.bind(v.as_slice()),
            Arg::Float(v) => query.bind(*v),
        };
    }
    query
}

pub async fn grid(
    db: &mut impl sn_store::Db,
    sql: &Sql,
    text: &str,
    values: usize,
) -> sn_store::StoreResult<Vec<(i32, Vec<f64>)>> {
    use sqlx::Row;
    let rows = bind_query(sqlx::query(text), &sql.args)
        .fetch_all(db.conn())
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let bucket: i32 = row.try_get::<Option<i32>, _>(0)?.unwrap_or(0);
        let mut cells = Vec::with_capacity(values);
        for i in 1..=values {
            cells.push(row.try_get::<Option<f64>, _>(i)?.unwrap_or(0.0));
        }
        out.push((bucket, cells));
    }
    Ok(out)
}

pub async fn single(
    db: &mut impl sn_store::Db,
    sql: &Sql,
    text: &str,
) -> sn_store::StoreResult<Vec<Option<f64>>> {
    use sqlx::Row;
    let row = bind_query(sqlx::query(text), &sql.args)
        .fetch_one(db.conn())
        .await?;
    let mut out = Vec::with_capacity(row.len());
    for i in 0..row.len() {
        out.push(row.try_get::<Option<f64>, _>(i)?);
    }
    Ok(out)
}

pub fn spread(n: usize, rows: &[(i32, Vec<f64>)], column: usize) -> Vec<f64> {
    let mut out = vec![0.0; n];
    for (bucket, cells) in rows {
        if let Ok(i) = usize::try_from(*bucket - 1)
            && i < n
        {
            out[i] += cells.get(column).copied().unwrap_or(0.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn шаги_делят_период_на_дни_недели_и_месяцы() {
        let october = Dates {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        };
        assert_eq!(starts(october, Step::Day).len(), 31);
        let weeks = starts(october, Step::Week);
        assert_eq!(weeks[0], d(2026, 10, 1));
        assert_eq!(weeks[1], d(2026, 10, 5));
        assert_eq!(weeks.len(), 5);
        let year = Dates {
            from: d(2025, 11, 15),
            to: d(2026, 10, 31),
        };
        let months = starts(year, Step::Month);
        assert_eq!(months[0], d(2025, 11, 15));
        assert_eq!(months[1], d(2025, 12, 1));
        assert_eq!(months.len(), 12);
    }

    #[test]
    fn шаг_сам_укрупняется_когда_точек_слишком_много() {
        let year = Dates {
            from: d(2025, 1, 1),
            to: d(2025, 12, 31),
        };
        assert_eq!(fit_step(year, Some(Step::Day)), Step::Week);
        assert_eq!(fit_step(year, None), Step::Month);
        let month = Dates {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        };
        assert_eq!(fit_step(month, None), Step::Day);
        let ten = Dates {
            from: d(2016, 1, 1),
            to: d(2026, 1, 1),
        };
        assert_eq!(fit_step(ten, Some(Step::Day)), Step::Month);
    }

    #[test]
    fn текущий_месяц_сравнивается_с_прошлым_на_тот_же_момент() {
        let offset = FixedOffset::east_opt(3 * 3600).unwrap();
        let dates = Dates {
            from: d(2026, 11, 1),
            to: d(2026, 11, 18),
        };
        let utc = |m: u32, day: u32, h: u32, min: u32| {
            Utc.with_ymd_and_hms(2026, m, day, h, min, 0).unwrap()
        };
        let live = Range {
            offset,
            dates,
            step: Step::Day,
            prev: previous(dates, Compare::Prev, false),
            today: d(2026, 11, 18),
            now: utc(11, 18, 17, 50),
        };
        assert_eq!(
            live.prev,
            Some(Dates {
                from: d(2026, 10, 1),
                to: d(2026, 10, 18)
            })
        );
        assert_eq!(
            live.prev_window(),
            Some((utc(9, 30, 21, 0), utc(10, 18, 17, 50)))
        );
        let bounds = live.prev_bounds().unwrap();
        assert_eq!(bounds.len(), 19);
        assert_eq!(bounds[17], utc(10, 17, 21, 0));
        assert_eq!(bounds[18], utc(10, 18, 17, 50));
        let closed = Range {
            today: d(2026, 11, 19),
            now: utc(11, 19, 9, 0),
            ..live
        };
        assert_eq!(
            closed.prev_window(),
            Some((utc(9, 30, 21, 0), utc(10, 18, 21, 0)))
        );
    }

    #[test]
    fn прошлый_период_по_календарю_той_же_длины_и_прошлый_год() {
        let october = Dates {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        };
        assert_eq!(
            previous(october, Compare::Prev, false),
            Some(Dates {
                from: d(2026, 9, 1),
                to: d(2026, 9, 30)
            })
        );
        let quarter = Dates {
            from: d(2026, 7, 1),
            to: d(2026, 9, 30),
        };
        assert_eq!(
            previous(quarter, Compare::Prev, false),
            Some(Dates {
                from: d(2026, 4, 1),
                to: d(2026, 6, 30)
            })
        );
        let so_far = Dates {
            from: d(2026, 3, 1),
            to: d(2026, 3, 30),
        };
        assert_eq!(
            previous(so_far, Compare::Prev, false),
            Some(Dates {
                from: d(2026, 2, 1),
                to: d(2026, 2, 28)
            })
        );
        let week = Dates {
            from: d(2026, 11, 12),
            to: d(2026, 11, 18),
        };
        assert_eq!(
            previous(week, Compare::Prev, false),
            Some(Dates {
                from: d(2026, 11, 5),
                to: d(2026, 11, 11)
            })
        );
        let across = Dates {
            from: d(2026, 10, 1),
            to: d(2026, 11, 18),
        };
        assert_eq!(
            previous(across, Compare::Prev, false),
            Some(Dates {
                from: d(2026, 8, 1),
                to: d(2026, 9, 18)
            })
        );
        let year = Dates {
            from: d(2025, 12, 1),
            to: d(2026, 11, 18),
        };
        assert_eq!(
            previous(year, Compare::Prev, false),
            Some(Dates {
                from: d(2024, 12, 1),
                to: d(2025, 11, 18)
            })
        );
        let mid = Dates {
            from: d(2026, 10, 5),
            to: d(2026, 11, 18),
        };
        assert_eq!(
            previous(mid, Compare::Prev, false),
            Some(Dates {
                from: d(2026, 8, 21),
                to: d(2026, 10, 4)
            })
        );
        assert_eq!(
            previous(october, Compare::Year, false),
            Some(Dates {
                from: d(2025, 10, 1),
                to: d(2025, 10, 31)
            })
        );
        assert_eq!(previous(october, Compare::None, false), None);
    }

    #[test]
    fn границы_дней_по_часовому_поясу_установки() {
        let msk = FixedOffset::east_opt(3 * 3600).unwrap();
        assert_eq!(
            midnight(d(2026, 10, 1), msk),
            Utc.with_ymd_and_hms(2026, 9, 30, 21, 0, 0).unwrap()
        );
        let b = bounds_of(
            Dates {
                from: d(2026, 10, 1),
                to: d(2026, 10, 2),
            },
            Step::Day,
            msk,
        );
        assert_eq!(b.len(), 3);
        assert_eq!(b[2], Utc.with_ymd_and_hms(2026, 10, 2, 21, 0, 0).unwrap());
    }

    #[test]
    fn фильтр_источника_разбирается() {
        assert_eq!(
            SourceFilter::parse("utm:tiktok/osen-shorts"),
            Some(SourceFilter::Utm {
                source: "tiktok".into(),
                campaign: Some("osen-shorts".into())
            })
        );
        assert_eq!(
            SourceFilter::parse("utm:vk"),
            Some(SourceFilter::Utm {
                source: "vk".into(),
                campaign: None
            })
        );
        assert_eq!(
            SourceFilter::parse("promo:12"),
            Some(SourceFilter::Promo("12".into()))
        );
        assert_eq!(SourceFilter::parse("ref"), Some(SourceFilter::Referral));
        assert_eq!(SourceFilter::parse("none"), Some(SourceFilter::Unlabeled));
        assert_eq!(SourceFilter::parse(""), None);
        assert_eq!(SourceFilter::parse("bad"), None);
    }

    #[test]
    fn неприменимые_фильтры_называются() {
        let f = Filters {
            provider: Some("yookassa".into()),
            source: Some(SourceFilter::Referral),
            ..Filters::default()
        };
        assert_eq!(
            f.ignored(&CLIENT_DIMS).as_deref(),
            Some("без фильтра по платёжке")
        );
        assert_eq!(f.ignored(&PAYMENT_DIMS), None);
    }
}
