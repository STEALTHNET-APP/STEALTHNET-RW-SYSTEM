use super::*;
use sn_domain::ids::{AdminId, TicketId};
use sn_domain::support::{MessageAuthor, TicketChannel};
use sn_store::{NewReply, NewTicket};
use std::collections::HashSet;

async fn open(h: &Http, client: ClientId, subject: &str, body: &str) -> TicketId {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.tickets()
        .open(&NewTicket {
            client_id: client,
            subscription_id: None,
            channel: TicketChannel::Bot,
            subject: Some(subject.into()),
            body: body.into(),
            ai: false,
        })
        .await
        .unwrap()
}

async fn imported_at(h: &Http, client: ClientId, subject: &str, at: DateTime<Utc>) -> TicketId {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tickets (client_id, channel, status, subject, last_author, created_at, updated_at) \
         VALUES ($1, 'unknown', 'open', $2, 'client', $3, $3) RETURNING id",
    )
    .bind(client.0)
    .bind(subject)
    .bind(at)
    .fetch_one(h.services().store().pool())
    .await
    .unwrap();
    TicketId(id)
}

async fn imported(h: &Http, client: ClientId, subject: &str, days_ago: i64) -> TicketId {
    imported_at(h, client, subject, Utc::now() - Duration::days(days_ago)).await
}

async fn moved_with(
    h: &Http,
    client: ClientId,
    subject: &str,
    times: &[DateTime<Utc>],
) -> TicketId {
    let id = imported_at(h, client, subject, times[0]).await;
    let pool = h.services().store().pool();
    for at in times {
        sqlx::query("INSERT INTO ticket_messages (ticket_id, author, body, created_at) VALUES ($1, 'client', $2, $3)")
            .bind(id.0)
            .bind(subject)
            .bind(at)
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "UPDATE tickets t SET last_message_id = (SELECT max(m.id) FROM ticket_messages m WHERE m.ticket_id = t.id), \
         waiting_message_id = (SELECT min(m.id) FROM ticket_messages m WHERE m.ticket_id = t.id), \
         last_message_at = (SELECT max(m.created_at) FROM ticket_messages m WHERE m.ticket_id = t.id) WHERE t.id = $1",
    )
    .bind(id.0)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn say(
    h: &Http,
    ticket: TicketId,
    author: MessageAuthor,
    admin: Option<AdminId>,
    close: bool,
) {
    let mut conn = h.services().store().conn().await.unwrap();
    conn.tickets()
        .reply(&NewReply {
            ticket_id: ticket,
            author,
            admin_id: admin,
            body: format!("{author:?}"),
            attachments: Vec::new(),
            close,
        })
        .await
        .unwrap();
}

async fn age(h: &Http, ticket: TicketId, days: i32) {
    let pool = h.services().store().pool();
    sqlx::query(
        "UPDATE ticket_messages SET created_at = now() - make_interval(days => $2) WHERE ticket_id = $1",
    )
    .bind(ticket.0)
    .bind(days)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE tickets SET created_at = now() - make_interval(days => $2), \
         last_message_at = (SELECT max(m.created_at) FROM ticket_messages m WHERE m.ticket_id = tickets.id) WHERE id = $1",
    )
        .bind(ticket.0)
        .bind(days)
        .execute(pool)
        .await
        .unwrap();
}

async fn scalar(h: &Http, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(h.services().store().pool())
        .await
        .unwrap()
}

fn ids_of(page: &Value) -> Vec<i64> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["ticket"]["id"].as_i64().unwrap())
        .collect()
}

async fn walk(h: &Http, token: &str, filter: &str, q: &str, limit: u32) -> (Vec<i64>, usize) {
    let mut ids = Vec::new();
    let mut pages = 0;
    let mut cursor: Option<(i64, i64)> = None;
    loop {
        let mut path = format!("/api/admin/support/tickets?filter={filter}&limit={limit}");
        if !q.is_empty() {
            path.push_str(&format!("&q={}", urlencoding(q)));
        }
        if let Some((key, id)) = cursor {
            path.push_str(&format!("&after={key}&after_id={id}"));
        }
        let (status, page) = h.get(&path, token).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        pages += 1;
        let got = ids_of(&page);
        assert!(got.len() <= limit as usize, "{page}");
        ids.extend(got.iter().copied());
        match (page["next"].as_i64(), page["next_id"].as_i64()) {
            (Some(key), Some(id)) => {
                assert_eq!(Some(&id), got.last(), "{page}");
                cursor = Some((key, id));
            }
            (None, None) => break,
            _ => panic!("next и next_id приходят вместе: {page}"),
        }
        assert!(pages < 100, "список не кончается");
    }
    (ids, pages)
}

#[tokio::test]
async fn список_обращений_отдаётся_частями_без_пропусков_и_повторов() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let admin = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.admins().list().await.unwrap()[0].id
    };
    let client = h.client(5_093_118_422).await;
    let same = Utc::now() - Duration::days(180);
    let mut moved = Vec::new();
    for i in 0..60 {
        moved.push(imported_at(&h, client, &format!("Перенесено {i}"), same).await);
    }
    let mut asked = Vec::new();
    for i in 0..90 {
        asked.push(
            open(
                &h,
                client,
                &format!("Вопрос {i}"),
                &format!("Не подключается, попытка {i}"),
            )
            .await,
        );
    }
    for ticket in &asked[..25] {
        say(&h, *ticket, MessageAuthor::Support, Some(admin), false).await;
    }
    for ticket in &asked[25..40] {
        say(&h, *ticket, MessageAuthor::Support, Some(admin), true).await;
    }

    let (status, first) = h
        .get("/api/admin/support/tickets?filter=waiting&limit=50", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["counts"]["waiting"], 110);
    assert_eq!(first["counts"]["answered"], 25);
    assert_eq!(first["counts"]["closed"], 15);
    assert_eq!(
        ids_of(&first),
        asked[40..].iter().rev().map(|t| t.0).collect::<Vec<_>>(),
        "сверху новые"
    );

    for (filter, total) in [("waiting", 110), ("answered", 25), ("closed", 15)] {
        let (ids, pages) = walk(&h, &owner, filter, "", 50).await;
        assert_eq!(ids.len(), total, "{filter}: {ids:?}");
        assert_eq!(
            ids.iter().copied().collect::<HashSet<_>>().len(),
            total,
            "{filter}: повторы"
        );
        assert_eq!(pages, total.div_ceil(50), "{filter}");
        let (status, whole) = h
            .get(
                &format!("/api/admin/support/tickets?filter={filter}&limit=200"),
                &owner,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{whole}");
        assert_eq!(ids, ids_of(&whole), "{filter}: порядок частями тот же");
        assert_eq!(whole["next"], Value::Null);
        assert_eq!(whole["next_id"], Value::Null);
    }
    let (waiting, _) = walk(&h, &owner, "waiting", "", 50).await;
    assert_eq!(
        waiting[..50],
        asked[40..].iter().rev().map(|t| t.0).collect::<Vec<_>>()[..]
    );
    assert_eq!(
        waiting[50..],
        moved.iter().rev().map(|t| t.0).collect::<Vec<_>>()[..],
        "у перенесённых одно и то же время: выше больший номер, ни одно не теряется на границе частей"
    );

    let (found, pages) = walk(&h, &owner, "waiting", "попытка 4", 4).await;
    let expected: Vec<i64> = asked
        .iter()
        .enumerate()
        .filter(|(i, _)| *i >= 40 && i.to_string().starts_with('4'))
        .map(|(_, t)| t.0)
        .rev()
        .collect();
    assert_eq!(found, expected);
    assert_eq!(pages, expected.len().div_ceil(4));
    let (found, _) = walk(&h, &owner, "closed", "Вопрос 3", 2).await;
    let expected: Vec<i64> = asked[30..40].iter().rev().map(|t| t.0).collect();
    assert_eq!(found, expected);

    let (_, answered) = h
        .get(
            "/api/admin/support/tickets?filter=answered&limit=10",
            &owner,
        )
        .await;
    let (key, id) = (
        answered["next"].as_i64().unwrap(),
        answered["next_id"].as_i64().unwrap(),
    );
    let (status, by_key) = h
        .get(
            &format!("/api/admin/support/tickets?filter=answered&limit=10&after={key}"),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{by_key}");
    let (_, by_both) = h
        .get(
            &format!(
                "/api/admin/support/tickets?filter=answered&limit=10&after={key}&after_id={id}"
            ),
            &owner,
        )
        .await;
    assert_eq!(
        ids_of(&by_key),
        ids_of(&by_both),
        "без after_id ключи без повторов листаются как раньше"
    );
    assert_eq!(ids_of(&by_key).len(), 10);
    h.db.cleanup().await;
}

#[tokio::test]
async fn отмеченные_обращения_закрываются_одной_записью_в_журнале_и_без_сообщений() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let readonly = h.admin("readonly@example.com", AdminRole::ReadOnly).await;
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let mut tickets = Vec::new();
    for subject in ["Не подключается", "Медленно", "Чек", "Возврат"]
    {
        let (status, body) = h
            .write(
                Method::POST,
                "/api/client/support/tickets",
                &token,
                json!({ "subject": subject, "body": format!("{subject}, помогите") }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        tickets.push(body["ticket_id"].as_i64().unwrap());
    }
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/support/tickets/{}/close", tickets[2]),
            &owner,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let jobs = scalar(&h, "SELECT count(*) FROM jobs").await;
    let messages = scalar(&h, "SELECT count(*) FROM ticket_messages").await;

    let close_many = "/api/admin/support/tickets/close-many";
    let (status, _) = h
        .write(
            Method::POST,
            close_many,
            &readonly,
            json!({ "ids": [tickets[0]] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = h
        .get(&format!("{close_many}?waiting_days=30"), &readonly)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    for (body, why) in [
        (json!({}), "ничего не указано"),
        (json!({ "ids": [] }), "пустой список"),
        (
            json!({ "ids": (1..=501).collect::<Vec<i64>>() }),
            "больше 500",
        ),
        (
            json!({ "ids": [tickets[0]], "waiting_days": 30 }),
            "и то и другое",
        ),
        (json!({ "waiting_days": 0 }), "ноль дней"),
        (json!({ "waiting_days": 3651 }), "больше десяти лет"),
    ] {
        let (status, answer) = h.write(Method::POST, close_many, &support, body).await;
        assert!(status.is_client_error(), "{why}: {status} {answer}");
    }

    let (status, done) = h
        .write(
            Method::POST,
            close_many,
            &support,
            json!({ "ids": [tickets[0], tickets[1], tickets[2], 987_654_321] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["closed"], 2);
    let status_of = |id: i64| {
        let h = &h;
        async move {
            sqlx::query_scalar::<_, String>("SELECT status FROM tickets WHERE id = $1")
                .bind(id)
                .fetch_one(h.services().store().pool())
                .await
                .unwrap()
        }
    };
    assert_eq!(status_of(tickets[0]).await, "closed");
    assert_eq!(status_of(tickets[1]).await, "closed");
    assert_eq!(status_of(tickets[3]).await, "open");
    assert_eq!(
        scalar(&h, "SELECT count(*) FROM jobs").await,
        jobs,
        "клиентам ничего не уходит"
    );
    assert_eq!(
        scalar(&h, "SELECT count(*) FROM ticket_messages").await,
        messages
    );
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) FROM audit_log WHERE action = 'ticket.bulk_closed'"
        )
        .await,
        1
    );
    let (status, journal) = h
        .get("/api/admin/audit?action=ticket.bulk_closed", &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{journal}");
    let entry = &journal["items"][0];
    assert_eq!(entry["title"], "Закрыто обращений в поддержку: 2");
    assert_eq!(entry["actor"]["email"], "support@example.com");
    let fields: Vec<(String, String)> = entry["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["label"].as_str().unwrap_or_default().to_string(),
                f["value"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(
        fields.contains(&("Количество".into(), "2".into())),
        "{fields:?}"
    );
    assert!(
        fields.contains(&(
            "Номера обращений".into(),
            format!("{}, {}", tickets[0], tickets[1])
        )),
        "{fields:?}"
    );

    let (_, list) = h
        .get("/api/admin/support/tickets?filter=closed", &support)
        .await;
    assert_eq!(list["counts"]["closed"], 3);
    assert_eq!(list["counts"]["waiting"], 1);
    let (_, mine) = h.get("/api/client/support/tickets", &token).await;
    assert_eq!(
        mine.as_array()
            .unwrap()
            .iter()
            .filter(|t| t["status"] == "closed")
            .count(),
        3
    );

    let (status, again) = h
        .write(
            Method::POST,
            close_many,
            &support,
            json!({ "ids": [tickets[0]] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["closed"], 0);
    assert_eq!(
        scalar(
            &h,
            "SELECT count(*) FROM audit_log WHERE action = 'ticket.bulk_closed'"
        )
        .await,
        1,
        "когда закрывать нечего, записи нет"
    );
    h.db.cleanup().await;
}

#[tokio::test]
async fn закрываются_только_обращения_где_ответа_ждут_дольше_срока() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let admin = {
        let mut conn = h.services().store().conn().await.unwrap();
        conn.admins().list().await.unwrap()[0].id
    };
    let client = h.client(5_093_118_422).await;
    let stale = open(&h, client, "Давно ждёт", "Не работает").await;
    age(&h, stale, 40).await;
    let wrote_again = open(&h, client, "Дописал недавно", "Не работает").await;
    age(&h, wrote_again, 40).await;
    say(&h, wrote_again, MessageAuthor::Client, None, false).await;
    let fresh = open(&h, client, "Свежий", "Не работает").await;
    let answered = open(&h, client, "Ответили", "Не работает").await;
    say(&h, answered, MessageAuthor::Support, Some(admin), false).await;
    age(&h, answered, 40).await;
    let moved = imported(&h, client, "Перенесено", 200).await;
    let closed = open(&h, client, "Уже закрыт", "Не работает").await;
    say(&h, closed, MessageAuthor::Support, Some(admin), true).await;
    age(&h, closed, 90).await;
    let jobs = scalar(&h, "SELECT count(*) FROM jobs").await;

    let (_, waiting) = h
        .get("/api/admin/support/tickets?filter=waiting", &owner)
        .await;
    assert_eq!(waiting["counts"]["waiting"], 4);

    let close_many = "/api/admin/support/tickets/close-many";
    let (status, count) = h
        .get(&format!("{close_many}?waiting_days=30"), &owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{count}");
    assert_eq!(count["count"], 2);
    let (_, count) = h
        .get(&format!("{close_many}?waiting_days=365"), &owner)
        .await;
    assert_eq!(count["count"], 0);
    let (status, _) = h.get(&format!("{close_many}?waiting_days=0"), &owner).await;
    assert!(status.is_client_error());

    let (status, done) = h
        .write(
            Method::POST,
            close_many,
            &owner,
            json!({ "waiting_days": 30 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["closed"], 2);
    let open_now: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM tickets WHERE status = 'open' ORDER BY id")
            .fetch_all(h.services().store().pool())
            .await
            .unwrap();
    let mut expected = vec![wrote_again.0, fresh.0, answered.0];
    expected.sort_unstable();
    assert_eq!(open_now, expected);
    let still_closed: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM tickets WHERE status = 'closed' ORDER BY id")
            .fetch_all(h.services().store().pool())
            .await
            .unwrap();
    let mut expected = vec![stale.0, moved.0, closed.0];
    expected.sort_unstable();
    assert_eq!(still_closed, expected);
    assert_eq!(scalar(&h, "SELECT count(*) FROM jobs").await, jobs);
    let details: Value =
        sqlx::query_scalar("SELECT details FROM audit_log WHERE action = 'ticket.bulk_closed'")
            .fetch_one(h.services().store().pool())
            .await
            .unwrap();
    assert_eq!(details["count"], 2);
    assert_eq!(details["waiting_days"], 30);
    let (_, count) = h
        .get(&format!("{close_many}?waiting_days=30"), &owner)
        .await;
    assert_eq!(count["count"], 0);
    h.db.cleanup().await;
}

#[tokio::test]
async fn обращения_идут_от_новых_к_старым_и_новое_сообщение_поднимает_наверх() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let client = h.client(5_093_118_422).await;
    let token = h.client_token(client).await;
    let other = h.client(5_093_118_423).await;
    let other_token = h.client_token(other).await;
    let now = Utc::now();
    let old = |days: i64| now - Duration::days(days);
    let a = moved_with(&h, client, "Давнее", &[old(197)]).await;
    let b = moved_with(&h, client, "Свежее из старой", &[old(120), old(79)]).await;
    let c = moved_with(&h, client, "Среднее", &[old(180)]).await;
    let d = imported_at(&h, client, "Без сообщений", old(150)).await;
    let e = moved_with(&h, client, "То же время", &[old(100)]).await;
    let f = moved_with(&h, client, "То же время", &[old(100)]).await;
    let mut fresh = Vec::new();
    for subject in ["Свежее X", "Свежее Y"] {
        let (status, body) = h
            .write(
                Method::POST,
                "/api/client/support/tickets",
                &other_token,
                json!({ "subject": subject, "body": "Не подключается" }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        fresh.push(TicketId(body["ticket_id"].as_i64().unwrap()));
    }
    let (x, y) = (fresh[0], fresh[1]);
    let list = |filter: &'static str| {
        let h = &h;
        let owner = owner.clone();
        async move {
            let (status, page) = h
                .get(
                    &format!("/api/admin/support/tickets?filter={filter}&limit=200"),
                    &owner,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{page}");
            ids_of(&page)
        }
    };
    let ids = |list: &[TicketId]| list.iter().map(|t| t.0).collect::<Vec<_>>();

    assert_eq!(
        list("waiting").await,
        ids(&[y, x, b, f, e, d, c, a]),
        "по времени последнего сообщения, у перенесённых — время старой панели"
    );

    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/client/support/tickets/{}", a.0),
            &token,
            json!({ "body": "Ответьте, пожалуйста" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list("waiting").await,
        ids(&[a, y, x, b, f, e, d, c]),
        "новое сообщение клиента поднимает обращение наверх"
    );

    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/admin/support/tickets/{}", b.0),
            &owner,
            json!({ "body": "Обновите подписку" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = h
        .write(
            Method::POST,
            &format!("/api/client/support/tickets/{}", x.0),
            &other_token,
            json!({ "body": "Есть кто-нибудь?" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let waiting = list("waiting").await;
    assert_eq!(waiting, ids(&[x, a, y, f, e, d, c]));
    assert_eq!(list("answered").await, ids(&[b]));

    let (paged, pages) = walk(&h, &owner, "waiting", "", 2).await;
    assert_eq!(paged, waiting, "частями вниз, к старым, тот же порядок");
    assert_eq!(pages, 4);
    let (found, _) = walk(&h, &owner, "waiting", "Свежее", 1).await;
    assert_eq!(found, ids(&[x, y]), "в поиске тот же порядок");

    let (_, all) = h
        .get(
            "/api/admin/support/tickets?filter=waiting&order=longest&limit=200",
            &owner,
        )
        .await;
    let since = |id: TicketId| {
        all["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["ticket"]["id"] == id.0)
            .map(|item| item["waiting_since"].as_str().map(str::to_string))
            .unwrap()
    };
    let parsed = |text: Option<String>| {
        text.map(|t| {
            DateTime::parse_from_rfc3339(&t)
                .unwrap()
                .with_timezone(&Utc)
        })
    };
    assert_eq!(
        parsed(since(d)).map(|at| at.timestamp_micros()),
        Some(old(150).timestamp_micros()),
        "без сообщений ждёт с создания"
    );
    assert_eq!(
        parsed(since(a)).map(|at| at.timestamp_micros()),
        Some(old(197).timestamp_micros()),
        "ждёт с первого неотвеченного сообщения"
    );
    for id in [c, d] {
        let (status, _) = h
            .write(
                Method::POST,
                &format!("/api/admin/support/tickets/{}/close", id.0),
                &owner,
                json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(
        list("closed").await,
        ids(&[d, c]),
        "закрытие не двигает обращение"
    );

    let (status, longest) = h
        .get(
            "/api/admin/support/tickets?filter=waiting&order=longest&limit=1",
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{longest}");
    assert_eq!(
        ids_of(&longest),
        ids(&[a]),
        "для главной — кто ждёт дольше всех"
    );
    let (status, _) = h
        .get(
            "/api/admin/support/tickets?filter=waiting&order=oldest",
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    h.db.cleanup().await;
}
