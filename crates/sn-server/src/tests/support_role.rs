use super::*;
use sn_remnawave::Node;

async fn remna_event(h: &Http, scope: &str, event: &str, data: Value) {
    let payload = json!({ "scope": scope, "event": event, "data": data });
    sqlx::query(
        "INSERT INTO remna_events (body_sha256, scope, event, occurred_at, payload) \
         VALUES (sha256(convert_to($1, 'UTF8')), $2, $3, now(), $4)",
    )
    .bind(payload.to_string())
    .bind(scope)
    .bind(event)
    .bind(&payload)
    .execute(h.services().store().pool())
    .await
    .unwrap();
}

fn menu_modules(me: &Value) -> Vec<String> {
    me["menu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["module"].as_str().unwrap().to_string())
        .collect()
}

fn feed_kinds(feed: &Value) -> Vec<String> {
    feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn поддержка_не_видит_utm_ноды_сервер_панели_и_деньги_на_главной() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let node: Node = serde_json::from_value(json!({
        "uuid": Uuid::from_u128(7), "name": "NL-7", "address": "198.51.100.27",
        "isConnected": true, "isDisabled": false, "isConnecting": false, "usersOnline": 3
    }))
    .unwrap();
    h.remna.set_nodes(vec![node]);
    h.client(5_093_118_422).await;
    remna_event(
        &h,
        "node",
        "node.connection_lost",
        json!({ "name": "NL-7" }),
    )
    .await;
    remna_event(
        &h,
        "user",
        "user.created",
        json!({ "username": "morozov_a" }),
    )
    .await;

    let (status, me) = h.get("/api/admin/me", &owner).await;
    assert_eq!(status, StatusCode::OK);
    let owner_menu = menu_modules(&me);
    assert!(owner_menu.contains(&"utm".to_string()), "{owner_menu:?}");
    assert!(owner_menu.contains(&"promo".to_string()), "{owner_menu:?}");
    let (status, me) = h.get("/api/admin/me", &support).await;
    assert_eq!(status, StatusCode::OK);
    let support_menu = menu_modules(&me);
    for hidden in ["utm", "promo", "broadcast", "referral", "analytics"] {
        assert!(
            !support_menu.contains(&hidden.to_string()),
            "{hidden}: {support_menu:?}"
        );
    }

    let (status, dashboard) = h.get("/api/admin/dashboard", &owner).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["remna"]["nodes_total"], 1);
    assert_eq!(dashboard["remna"]["nodes"][0]["name"], "NL-7");
    assert!(dashboard["server"].is_object(), "{dashboard}");
    if dashboard["today"].is_object() {
        assert!(dashboard["today"]["payments"].is_number());
        assert!(dashboard["today"]["new_clients_paid"].is_number());
        let days = dashboard["days"].as_array().expect("ряд по дням");
        assert_eq!(days.len(), 14, "{dashboard}");
        let last = &days[13];
        assert_eq!(last["day"], dashboard["revenue"]["by_day"][13]["day"]);
        assert_eq!(last["revenue"], dashboard["revenue"]["today"]);
        assert_eq!(last["payments"], dashboard["today"]["payments"]);
        assert_eq!(last["new_clients"], dashboard["today"]["new_clients"]);
        assert_eq!(
            last["new_clients_paid"],
            dashboard["today"]["new_clients_paid"]
        );
        assert!(dashboard["revenue"]["week_before"]["minor"].is_number());
    }
    assert!(dashboard["orders_review"].is_number());

    let (status, dashboard) = h.get("/api/admin/dashboard", &support).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert!(dashboard["revenue"].is_null());
    assert!(dashboard["server"].is_null(), "{dashboard}");
    assert!(dashboard["remna_error"].is_null());
    assert_eq!(dashboard["remna"]["nodes"], json!([]));
    assert_eq!(dashboard["remna"]["nodes_total"], 0);
    assert!(dashboard["remna"]["online_now"].is_number());
    if dashboard["today"].is_object() {
        assert!(dashboard["today"]["payments"].is_null());
        assert!(dashboard["today"]["new_clients_paid"].is_null());
        assert!(dashboard["today"]["new_clients"].is_number());
        let days = dashboard["days"].as_array().expect("ряд по дням");
        assert_eq!(days.len(), 14, "{dashboard}");
        for day in days {
            assert!(day["new_clients"].is_number(), "{day}");
            for hidden in ["revenue", "payments", "new_clients_paid"] {
                assert!(day[hidden].is_null(), "{hidden}: {day}");
            }
        }
    }
    for field in [
        "orders_review",
        "stars_review",
        "refund_actions_failed",
        "charges_review",
        "webhooks",
    ] {
        assert!(dashboard[field].is_null(), "{field}: {dashboard}");
    }
    assert!(!dashboard.to_string().contains("NL-7"));

    let (_, feed) = h.get("/api/admin/feed?kind=all&limit=20", &owner).await;
    let kinds = feed_kinds(&feed);
    assert!(
        kinds.contains(&"node.connection_lost".to_string()),
        "{kinds:?}"
    );
    assert!(kinds.contains(&"user.created".to_string()), "{kinds:?}");
    let (_, feed) = h.get("/api/admin/feed?kind=all&limit=20", &support).await;
    let kinds = feed_kinds(&feed);
    assert!(!kinds.iter().any(|k| k.starts_with("node.")), "{kinds:?}");
    assert!(kinds.contains(&"user.created".to_string()), "{kinds:?}");
    for kind in ["nodes", "highlights"] {
        let (status, feed) = h
            .get(&format!("/api/admin/feed?kind={kind}&limit=20"), &support)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !feed_kinds(&feed).iter().any(|k| k.starts_with("node.")),
            "{kind}: {feed}"
        );
    }
    h.db.cleanup().await;
}
