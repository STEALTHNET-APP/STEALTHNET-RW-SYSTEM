mod clients;
mod days;
mod lists;
mod money;
mod payments;
mod retention;
mod sources;
mod support;
mod tariffs;

use super::labels;
use super::metric::{Detail, Metric, TabReport};
use super::query::{Compare, Filters};
use super::tab::Ctx;
use super::{TabQuery, range, spans};
use crate::context::{MethodTitle, PaymentTitle};
use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_json::{Value, json};
use sn_domain::money::Currency;
use sn_store::{Db, Store};

pub fn utc(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
}

pub fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

pub fn now() -> DateTime<Utc> {
    utc(2026, 4, 15, 12)
}

pub fn msk() -> FixedOffset {
    FixedOffset::east_opt(3 * 3600).unwrap()
}

pub fn titles() -> Vec<PaymentTitle> {
    vec![PaymentTitle {
        provider: "yookassa".into(),
        title: "ЮKassa".into(),
        button: "Банковская карта".into(),
        methods: vec![MethodTitle {
            code: "sbp".into(),
            title: "СБП".into(),
        }],
    }]
}

pub struct Seed {
    pub store: Store,
}

pub struct Buy<'a> {
    pub client: i64,
    pub subscription: Option<i64>,
    pub kind: &'a str,
    pub at: DateTime<Utc>,
    pub tariff: Option<i64>,
    pub days: i64,
    pub total: i64,
    pub promo: i64,
    pub provider: &'a str,
    pub method: Option<&'a str>,
    pub currency: &'a str,
    pub amount: i64,
    pub channel: Option<&'a str>,
    pub intent: Option<Value>,
}

impl Default for Buy<'_> {
    fn default() -> Self {
        Buy {
            client: 0,
            subscription: None,
            kind: "new",
            at: utc(2026, 3, 1, 9),
            tariff: None,
            days: 30,
            total: 0,
            promo: 0,
            provider: "yookassa",
            method: None,
            currency: "RUB",
            amount: 0,
            channel: None,
            intent: None,
        }
    }
}

impl Seed {
    pub async fn exec(&self, sql: &str) {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::raw_sql(sql).execute(conn.conn()).await.unwrap();
    }

    pub async fn scalar(&self, sql: &str) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query_scalar(sql)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    }

    pub async fn category(&self, name: &str) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query_scalar("INSERT INTO tariff_categories (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    }

    pub async fn tariff(&self, category: i64, name: &str) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query_scalar("INSERT INTO tariffs (category_id, name) VALUES ($1, $2) RETURNING id")
            .bind(category)
            .bind(name)
            .fetch_one(conn.conn())
            .await
            .unwrap()
    }

    pub async fn client(
        &self,
        created: DateTime<Utc>,
        channel: &str,
        utm: Option<(&str, &str)>,
    ) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO clients (registration_source, created_at) VALUES ($1, $2) RETURNING id",
        )
        .bind(channel)
        .bind(created)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        if let Some((source, campaign)) = utm {
            sqlx::query(
                "INSERT INTO client_utm (client_id, source, campaign, start_param, registration_source, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(source)
            .bind(campaign)
            .bind(format!("s_{source}__k_{campaign}"))
            .bind(channel)
            .bind(created)
            .execute(conn.conn())
            .await
            .unwrap();
        }
        id
    }

    pub async fn identity(&self, client: i64, kind: &str, value: &str) {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query("INSERT INTO client_identities (client_id, kind, value) VALUES ($1, $2, $3)")
            .bind(client)
            .bind(kind)
            .bind(value)
            .execute(conn.conn())
            .await
            .unwrap();
    }

    pub async fn subscription(&self, client: i64, kind: &str, tariff: Option<i64>) -> i64 {
        let mut conn = self.store.conn().await.unwrap();
        sqlx::query_scalar(
            "INSERT INTO subscriptions (client_id, kind, tariff_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(client)
        .bind(kind)
        .bind(tariff)
        .fetch_one(conn.conn())
        .await
        .unwrap()
    }

    pub async fn buy(&self, b: Buy<'_>) -> (i64, i64) {
        let mut lines = Vec::new();
        if let Some(t) = b.tariff {
            lines.push(json!({
                "kind": {"type": "period", "tariff_id": t, "period_id": 1, "days": b.days, "months": 1},
                "label": "Тариф",
                "amount": {"minor": b.total + b.promo, "currency": "RUB"}
            }));
        }
        if b.promo > 0 {
            lines.push(json!({
                "kind": {"type": "adjustment", "source": {"type": "promo", "code": "AUTUMN", "promo_id": null}},
                "label": "Промокод AUTUMN",
                "amount": {"minor": -b.promo, "currency": "RUB"}
            }));
        }
        let quote = json!({"days": b.days, "lines": lines});
        let snapshot = b
            .tariff
            .map(|t| json!({"tariff_id": t, "days": b.days, "name": "Тариф"}));
        let intent = b.intent.clone().unwrap_or_else(|| json!({"type": "trial"}));
        let mut conn = self.store.conn().await.unwrap();
        let order: i64 = sqlx::query_scalar(
            "INSERT INTO orders (client_id, subscription_id, kind, status, intent, quote, snapshot, currency, \
               total_minor, stars_total, created_at, paid_at, channel) \
             VALUES ($1, $2, $3, 'fulfilled', $4, $5, $6, 'RUB', $7, $8, $9, $9, $10) RETURNING id",
        )
        .bind(b.client)
        .bind(b.subscription)
        .bind(b.kind)
        .bind(&intent)
        .bind(&quote)
        .bind(&snapshot)
        .bind(b.total)
        .bind(if b.currency == "XTR" { Some(b.amount) } else { None })
        .bind(b.at)
        .bind(b.channel)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        let payment: i64 = sqlx::query_scalar(
            "INSERT INTO payments (order_id, client_id, provider, pay_method, amount_minor, currency, status, created_at, paid_at) \
             VALUES ($1, $2, $3, $4, $5, $6, 'paid', $7 - interval '3 minutes', $7) RETURNING id",
        )
        .bind(order)
        .bind(b.client)
        .bind(b.provider)
        .bind(b.method)
        .bind(b.amount)
        .bind(b.currency)
        .bind(b.at)
        .fetch_one(conn.conn())
        .await
        .unwrap();
        (order, payment)
    }
}

pub async fn ctx_report(store: &Store, tab: &str, query: TabQuery) -> TabReport {
    spans::refresh(store, Currency::Rub, Some(chrono::Duration::zero()))
        .await
        .unwrap();
    let names = {
        let mut conn = store.conn().await.unwrap();
        labels::names(&mut conn).await.unwrap()
    };
    let r = range(&query, msk(), now()).unwrap();
    let c = Ctx {
        store,
        range: r,
        filters: query.filters(),
        currency: Currency::Rub,
        titles: titles(),
        names,
    };
    let groups = match tab {
        "money" => super::money::build(&c).await.unwrap(),
        "clients" => super::clients::build(&c).await.unwrap(),
        "retention" => super::retention::build(&c).await.unwrap(),
        "payments" => super::payments::build(&c).await.unwrap(),
        "sources" => super::sources::build(&c).await.unwrap(),
        "tariffs" => super::tariffs::build(&c).await.unwrap(),
        "economy" => {
            let core = crate::catalog::testing::TestCore::new(store);
            super::economy::build(&c, core.as_ref()).await.unwrap()
        }
        _ => super::support::build(&c).await.unwrap(),
    };
    TabReport {
        tab: "test",
        currency: Currency::Rub,
        range: super::metric::RangeInfo::of(&c.range),
        groups,
        notes: Vec::new(),
        preparing: false,
    }
}

pub fn month(m: u32, compare: Compare) -> TabQuery {
    let from = day(2026, m, 1);
    let to = from
        .checked_add_months(chrono::Months::new(1))
        .unwrap()
        .pred_opt()
        .unwrap();
    TabQuery {
        from: Some(from),
        to: Some(to.min(day(2026, 4, 15))),
        compare: Some(compare),
        ..TabQuery::default()
    }
}

pub fn value(r: &TabReport, key: &str) -> f64 {
    metric(r, key).value.unwrap_or(f64::NAN)
}

pub fn metric<'a>(r: &'a TabReport, key: &str) -> &'a Metric {
    r.metric(key)
        .unwrap_or_else(|| panic!("нет показателя {key}"))
}

pub fn breakdown(r: &TabReport, key: &str, by: &str) -> Vec<(String, f64)> {
    match &metric(r, key).detail {
        Detail::Series { breakdowns, .. } => breakdowns
            .iter()
            .find(|b| b.key == by)
            .map(|b| b.rows.iter().map(|r| (r.key.clone(), r.value)).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn table(r: &TabReport, key: &str) -> Vec<(String, Vec<Option<f64>>)> {
    match &metric(r, key).detail {
        Detail::Table { rows, .. } => rows
            .iter()
            .map(|r| (r.label.clone(), r.cells.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn filtered(query: TabQuery, f: impl FnOnce(&mut TabQuery)) -> TabQuery {
    let mut q = query;
    f(&mut q);
    q
}

#[test]
fn конец_периода_позже_сегодняшнего_дня_панели_подрезается_а_не_отклоняется() {
    let ahead = TabQuery {
        from: Some(day(2026, 4, 1)),
        to: Some(day(2026, 4, 16)),
        ..TabQuery::default()
    };
    let r = range(&ahead, msk(), now()).unwrap();
    assert_eq!(
        (r.dates.from, r.dates.to),
        (day(2026, 4, 1), day(2026, 4, 15))
    );
    let tomorrow = TabQuery {
        from: Some(day(2026, 4, 16)),
        to: Some(day(2026, 4, 16)),
        ..TabQuery::default()
    };
    let r = range(&tomorrow, msk(), now()).unwrap();
    assert_eq!(
        (r.dates.from, r.dates.to),
        (day(2026, 4, 15), day(2026, 4, 15))
    );
    let inverted = TabQuery {
        from: Some(day(2026, 4, 10)),
        to: Some(day(2026, 4, 5)),
        ..TabQuery::default()
    };
    assert!(range(&inverted, msk(), now()).is_err());
}
