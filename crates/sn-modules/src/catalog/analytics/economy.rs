use super::labels;
use super::metric::{Column, Detail, Group, Metric, Row, Unit, num, ratio, share};
use super::query::{Sql, grid, rows};
use super::tab::Ctx;
use crate::context::{CoreApi, HostingPayment, NodeOnline, NodeTraffic};
use chrono::{DateTime, Utc};
use sn_store::StoreResult;
use std::collections::HashMap;
use uuid::Uuid;

const TB: f64 = 1_000_000_000_000.0;

fn row(label: String, sub: Option<String>, cells: Vec<Option<f64>>) -> Row {
    Row {
        label,
        sub,
        cells,
        list: None,
        children: Vec::new(),
    }
}

async fn peaks(c: &Ctx<'_>) -> StoreResult<Vec<(Uuid, String, i64, f64)>> {
    let mut conn = c.store.conn().await?;
    Ok(sqlx::query_as(
        "SELECT node_uuid, max(name), max(online)::bigint, avg(online)::float8 FROM analytics_node_samples \
         WHERE at >= $1 AND at < $2 GROUP BY 1 ORDER BY 3 DESC",
    )
    .bind(c.range.start())
    .bind(c.range.end())
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await?)
}

async fn clients_by_node(c: &Ctx<'_>) -> StoreResult<Vec<(Option<Uuid>, i64)>> {
    let mut sql = Sql::default();
    let now = sql.time(c.range.now);
    let f = c.filters.client(&mut sql, "s.client_id");
    let text = format!(
        "SELECT s.last_node_uuid, count(*)::bigint FROM subscriptions s \
         WHERE s.ended_at IS NULL AND (s.expire_at IS NULL OR s.expire_at > {now}) AND {f} GROUP BY 1 ORDER BY 2 DESC"
    );
    let mut conn = c.store.conn().await?;
    rows(&mut conn, &sql, &text).await
}

type NodeEvent = (Option<String>, Option<String>, String, DateTime<Utc>);

async fn downtime(c: &Ctx<'_>) -> StoreResult<Vec<(String, i64, i64)>> {
    let mut conn = c.store.conn().await?;
    let events: Vec<NodeEvent> = sqlx::query_as(
        "SELECT subject_id, payload #>> '{data,name}', event, occurred_at FROM remna_events \
         WHERE subject_kind = 'node' AND scope = 'node' AND event IN ('node.connection_lost', 'node.connection_restored') \
           AND occurred_at >= $1 AND occurred_at < $2 ORDER BY occurred_at",
    )
    .bind(c.range.start() - chrono::Duration::days(30))
    .bind(c.range.end())
    .fetch_all(sn_store::Db::conn(&mut conn))
    .await?;
    let start = c.range.start();
    let end = c.range.end().min(c.range.now);
    let mut down: HashMap<String, (String, DateTime<Utc>)> = HashMap::new();
    let mut total: HashMap<String, (String, i64, i64)> = HashMap::new();
    for (id, name, event, at) in events {
        let id = id.unwrap_or_default();
        let name = name.unwrap_or_else(|| id.clone());
        if event == "node.connection_lost" {
            down.entry(id).or_insert((name, at));
        } else if let Some((n, since)) = down.remove(&id) {
            let a = since.max(start);
            if at > a {
                let e = total.entry(id).or_insert((n, 0, 0));
                e.1 += (at - a).num_seconds();
                e.2 += 1;
            }
        }
    }
    for (id, (n, since)) in down {
        let a = since.max(start);
        if end > a {
            let e = total.entry(id).or_insert((n, 0, 0));
            e.1 += (end - a).num_seconds();
            e.2 += 1;
        }
    }
    let mut out: Vec<(String, i64, i64)> = total.into_values().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

struct MoneySide {
    net: f64,
    referral: f64,
    prizes: f64,
    paying: f64,
    bonuses: f64,
}

async fn money_side(c: &Ctx<'_>) -> StoreResult<MoneySide> {
    let mut sql = Sql::default();
    let s = sql.time(c.range.start());
    let e = sql.time(c.range.end());
    let cur = sql.text(c.currency.code());
    let f = c.filters.facts(&mut sql, "f");
    let fs = c.filters.spans(&mut sql, "sp");
    let text = format!(
        "SELECT 0, \
           (SELECT COALESCE(sum(f.amount_minor - f.refunded_minor), 0) FROM analytics_payments f \
              WHERE f.paid_at >= {s} AND f.paid_at < {e} AND NOT f.stars AND f.provider <> 'balance' AND {f})::float8, \
           (SELECT COALESCE(sum(r.amount_minor - r.reversed_minor), 0) FROM referral_credits r \
              WHERE r.credited_at >= {s} AND r.credited_at < {e} AND r.currency = {cur})::float8, \
           (SELECT COALESCE(sum(b.delta_minor), 0) FROM balance_entries b WHERE b.kind IN ('contest', 'roulette') \
              AND b.delta_minor > 0 AND b.currency = {cur} AND b.created_at >= {s} AND b.created_at < {e})::float8, \
           (SELECT count(DISTINCT sp.client_id) FROM analytics_spans sp \
              WHERE sp.starts_at < {e} AND sp.ends_at > {s} AND {fs})::float8, \
           (SELECT COALESCE(sum(b.delta_minor), 0) FROM balance_entries b \
              WHERE b.kind IN ('topup_bonus', 'topup_bonus_reversal') \
              AND b.currency = {cur} AND b.created_at >= {s} AND b.created_at < {e})::float8"
    );
    let mut conn = c.store.conn().await?;
    let r = grid(&mut conn, &sql, &text, 5).await?;
    let v = r
        .first()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| vec![0.0; 5]);
    Ok(MoneySide {
        net: v[0],
        referral: v[1],
        prizes: v[2],
        paying: v[3],
        bonuses: v[4],
    })
}

pub async fn build(c: &Ctx<'_>, core: &dyn CoreApi) -> StoreResult<Vec<Group>> {
    let (peak, by_node, down, side) =
        tokio::try_join!(peaks(c), clients_by_node(c), downtime(c), money_side(c))?;
    let MoneySide {
        net,
        referral,
        prizes,
        paying,
        bonuses,
    } = side;
    let online: Result<Vec<NodeOnline>, _> = core.nodes_online().await;
    let traffic: Result<Vec<NodeTraffic>, _> = core
        .nodes_traffic(c.range.dates.from, c.range.dates.to)
        .await;
    let hosting: Result<Vec<HostingPayment>, _> = core.hosting_payments().await;
    let unreachable = "Remnawave не ответила";
    let (start, end) = (c.range.start(), c.range.end());
    let paid_hosting: Option<Vec<HostingPayment>> = hosting.as_ref().ok().map(|h| {
        h.iter()
            .filter(|p| p.at >= start && p.at < end)
            .cloned()
            .collect()
    });
    let hosting_sum: Option<f64> = paid_hosting
        .as_ref()
        .map(|h| h.iter().map(|p| num(p.amount_minor)).sum());
    let mut by_provider: Vec<(String, f64, f64)> = Vec::new();
    for p in paid_hosting.iter().flatten() {
        match by_provider.iter_mut().find(|x| x.0 == p.provider) {
            Some(x) => {
                x.1 += num(p.amount_minor);
                x.2 += 1.0;
            }
            None => by_provider.push((p.provider.clone(), num(p.amount_minor), 1.0)),
        }
    }
    by_provider.sort_by(|a, b| b.1.total_cmp(&a.1));
    let hosting_metric = Metric::new("hosting", "Оплата хостинга", Unit::Money, hosting_sum)
        .hint("Записи из «Инфраструктура → Оплата»")
        .note_opt(hosting.is_err().then(|| unreachable.to_string()))
        .detail(Detail::Table {
            columns: vec![
                Column::new("Сумма", Unit::Money),
                Column::new("Оплат", Unit::Count),
            ],
            rows: by_provider
                .into_iter()
                .map(|(n, s, k)| row(n, None, vec![Some(s), Some(k)]))
                .collect(),
            note: None,
        });
    let traffic_sum: Option<f64> = traffic
        .as_ref()
        .ok()
        .map(|t| t.iter().map(|n| num(n.bytes)).sum());
    let traffic_metric = Metric::new("node_traffic", "Трафик по нодам", Unit::Bytes, traffic_sum)
        .note_opt(traffic.is_err().then(|| unreachable.to_string()))
        .detail(Detail::Table {
            columns: vec![
                Column::new("Трафик", Unit::Bytes),
                Column::new("Доля", Unit::Percent),
            ],
            rows: traffic
                .as_ref()
                .map(|t| {
                    t.iter()
                        .map(|n| {
                            row(
                                n.name.clone(),
                                None,
                                vec![
                                    Some(num(n.bytes)),
                                    share(num(n.bytes), traffic_sum.unwrap_or(0.0)),
                                ],
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            note: None,
        });
    let names: HashMap<Uuid, String> = online
        .as_ref()
        .map(|o| o.iter().map(|n| (n.uuid, n.name.clone())).collect())
        .unwrap_or_default();
    let online_now: Option<f64> = online
        .as_ref()
        .ok()
        .map(|o| o.iter().map(|n| num(n.online)).sum());
    let mut online_rows: Vec<Row> = Vec::new();
    if let Ok(list) = &online {
        for n in list {
            let p = peak.iter().find(|p| p.0 == n.uuid);
            online_rows.push(row(
                n.name.clone(),
                (!n.connected).then(|| "не в сети".to_string()),
                vec![
                    Some(num(n.online)),
                    p.map(|p| num(p.2)),
                    p.map(|p| p.3.round()),
                ],
            ));
        }
    }
    for (uuid, name, max, avg) in &peak {
        if !online_rows.iter().any(|r| &r.label == name) && !names.contains_key(uuid) {
            online_rows.push(row(
                name.clone(),
                Some("сейчас нет в Remnawave".into()),
                vec![None, Some(num(*max)), Some(avg.round())],
            ));
        }
    }
    let first_sample: Option<DateTime<Utc>> = {
        let mut conn = c.store.conn().await?;
        sqlx::query_scalar("SELECT min(at) FROM analytics_node_samples")
            .fetch_one(sn_store::Db::conn(&mut conn))
            .await?
    };
    let online_metric = Metric::new("online", "Онлайн сейчас", Unit::Count, online_now)
        .note_opt(if online.is_err() {
            Some(unreachable.to_string())
        } else {
            first_sample.map(|at| {
                format!(
                    "пики — с {}",
                    labels::date_text(super::query::local_day(at, c.range.offset))
                )
            })
        })
        .detail(Detail::Table {
            columns: vec![
                Column::new("Сейчас", Unit::Count),
                Column::new("Пик за период", Unit::Count),
                Column::new("В среднем", Unit::Count),
            ],
            rows: online_rows,
            note: Some("Пики и среднее — по замерам раз в 10 минут".into()),
        });
    let node_rows: Vec<Row> = by_node
        .iter()
        .map(|(uuid, n)| {
            let label = match uuid {
                Some(u) => names
                    .get(u)
                    .cloned()
                    .or_else(|| peak.iter().find(|p| &p.0 == u).map(|p| p.1.clone()))
                    .unwrap_or_else(|| u.to_string()),
                None => "Ещё не подключались".into(),
            };
            row(label, None, vec![Some(num(*n))])
        })
        .collect();
    let clients_nodes = Metric::new(
        "node_clients",
        "Клиенты по нодам",
        Unit::Count,
        Some(by_node.iter().map(|r| num(r.1)).sum()),
    )
    .note("по последнему подключению")
    .detail(Detail::Table {
        columns: vec![Column::new("Подписок", Unit::Count)],
        rows: node_rows,
        note: None,
    });
    let down_total: f64 = down.iter().map(|d| num(d.1)).sum();
    let down_metric = Metric::new("downtime", "Простои нод", Unit::Seconds, Some(down_total))
        .note(format!("{} раз", down.iter().map(|d| d.2).sum::<i64>()))
        .detail(Detail::Table {
            columns: vec![
                Column::new("Не отвечала", Unit::Seconds),
                Column::new("Раз", Unit::Count),
            ],
            rows: down
                .iter()
                .map(|(n, s, k)| row(n.clone(), None, vec![Some(num(*s)), Some(num(*k))]))
                .collect(),
            note: Some("По событиям Remnawave о потере связи с нодой".into()),
        });
    let per_client = hosting_sum.and_then(|h| ratio(h, paying));
    let per_tb = match (hosting_sum, traffic_sum) {
        (Some(h), Some(t)) if t > 0.0 => Some((h / (t / TB)).round()),
        _ => None,
    };
    let margin = net - hosting_sum.unwrap_or(0.0) - referral - prizes - bonuses;
    let mut margin_rows = vec![
        row("Выручка без возвратов".into(), None, vec![Some(net)]),
        row("Хостинг".into(), None, vec![hosting_sum.map(|h| -h)]),
        row("Начислено рефералам".into(), None, vec![Some(-referral)]),
        row("Денежные призы".into(), None, vec![Some(-prizes)]),
    ];
    if bonuses != 0.0 {
        margin_rows.push(row(
            "Бонусы за пополнения".into(),
            None,
            vec![Some(-bonuses)],
        ));
    }
    margin_rows.push(row("Маржа".into(), None, vec![Some(margin)]));
    let cost_client = Metric::new(
        "cost_client",
        "Хостинг на платящего",
        Unit::Money,
        per_client,
    )
    .note(format!("{} платящих", paying as i64))
    .detail(Detail::None);
    let cost_tb =
        Metric::new("cost_tb", "Хостинг на 1 ТБ трафика", Unit::Money, per_tb).detail(Detail::None);
    let margin_metric = Metric::new("margin", "Маржа", Unit::Money, Some(margin))
        .note_opt(share(margin, net).map(|s| format!("{} выручки", super::metric::pct_text(s))))
        .hint("Без комиссий платёжек: панель их не знает")
        .detail(Detail::Table {
            columns: vec![Column::new("Сумма", Unit::Money)],
            rows: margin_rows,
            note: None,
        });
    Ok(vec![
        Group::new(
            None,
            vec![hosting_metric, cost_client, cost_tb, margin_metric],
        ),
        Group::new(
            Some("Ноды"),
            vec![online_metric, traffic_metric, clients_nodes, down_metric],
        ),
    ])
}
