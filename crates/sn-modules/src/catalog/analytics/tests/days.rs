use super::*;
use crate::catalog::analytics::metric::Point;
use crate::catalog::analytics::query::{Dates, Step, previous};
use sn_store::testing::test_store;

fn at(d: u32, h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, d, h, m, 0).unwrap()
}

fn one_day(d: u32) -> TabQuery {
    TabQuery {
        from: Some(day(2026, 4, d)),
        to: Some(day(2026, 4, d)),
        compare: Some(Compare::Prev),
        ..TabQuery::default()
    }
}

fn series(r: &TabReport, key: &str) -> (Vec<Point>, Vec<Point>) {
    match &metric(r, key).detail {
        Detail::Series { points, prev, .. } => (points.clone(), prev.clone()),
        other => panic!("{key}: {other:?}"),
    }
}

#[test]
fn один_день_и_скользящие_дни_сравниваются_с_предыдущими_днями() {
    let first = Dates {
        from: day(2026, 4, 1),
        to: day(2026, 4, 1),
    };
    assert_eq!(
        previous(first, Compare::Prev, false),
        Some(Dates {
            from: day(2026, 3, 31),
            to: day(2026, 3, 31)
        })
    );
    let month = Dates {
        from: day(2026, 4, 1),
        to: day(2026, 4, 10),
    };
    assert_eq!(
        previous(month, Compare::Prev, false),
        Some(Dates {
            from: day(2026, 3, 1),
            to: day(2026, 3, 10)
        })
    );
    let seven = Dates {
        from: day(2026, 4, 1),
        to: day(2026, 4, 7),
    };
    assert_eq!(
        previous(seven, Compare::Prev, true),
        Some(Dates {
            from: day(2026, 3, 25),
            to: day(2026, 3, 31)
        })
    );
    assert_eq!(
        previous(seven, Compare::Prev, false),
        Some(Dates {
            from: day(2026, 3, 1),
            to: day(2026, 3, 7)
        })
    );
    let rolling = TabQuery {
        from: Some(day(2026, 4, 9)),
        to: Some(day(2026, 4, 15)),
        rolling: Some(true),
        ..TabQuery::default()
    };
    assert_eq!(
        range(&rolling, msk(), now()).unwrap().prev,
        Some(Dates {
            from: day(2026, 4, 2),
            to: day(2026, 4, 8)
        })
    );
    let r = range(&one_day(14), msk(), now()).unwrap();
    assert_eq!(
        (r.dates.from, r.dates.to, r.step),
        (day(2026, 4, 14), day(2026, 4, 14), Step::Day)
    );
    assert_eq!(
        r.prev,
        Some(Dates {
            from: day(2026, 4, 13),
            to: day(2026, 4, 13)
        })
    );
    assert!(!r.live());
}

#[tokio::test]
async fn вчера_и_сегодня_считаются_по_суткам_панели() {
    let Some(db) = test_store().await else {
        return;
    };
    let seed = Seed {
        store: db.store.clone(),
    };
    let early = seed.client(at(13, 20, 30), "bot", None).await;
    let night = seed.client(at(13, 21, 30), "bot", None).await;
    seed.client(at(14, 20, 50), "web", None).await;
    seed.client(at(14, 21, 10), "web", None).await;
    for (client, when, amount) in [
        (early, at(13, 20, 30), 10_000),
        (night, at(13, 21, 30), 20_000),
        (night, at(14, 20, 30), 30_000),
        (night, at(14, 21, 30), 40_000),
        (early, at(15, 11, 0), 5_000),
    ] {
        seed.buy(Buy {
            client,
            at: when,
            total: amount,
            amount,
            ..Buy::default()
        })
        .await;
    }

    let r = ctx_report(&db.store, "money", one_day(14)).await;
    assert_eq!(
        (r.range.from, r.range.to),
        (day(2026, 4, 14), day(2026, 4, 14))
    );
    assert_eq!(value(&r, "revenue"), 50_000.0);
    assert_eq!(metric(&r, "revenue").prev, Some(10_000.0));
    let (points, prev) = series(&r, "revenue");
    assert_eq!(
        points,
        vec![Point {
            at: day(2026, 4, 14),
            value: 50_000.0
        }]
    );
    assert_eq!(
        prev,
        vec![Point {
            at: day(2026, 4, 13),
            value: 10_000.0
        }]
    );

    let r = ctx_report(&db.store, "clients", one_day(14)).await;
    assert_eq!(value(&r, "registrations"), 2.0);
    assert_eq!(metric(&r, "registrations").prev, Some(1.0));

    let r = ctx_report(&db.store, "money", one_day(15)).await;
    assert_eq!(value(&r, "revenue"), 45_000.0);
    assert_eq!(metric(&r, "revenue").prev, Some(20_000.0));
    db.cleanup().await;
}
