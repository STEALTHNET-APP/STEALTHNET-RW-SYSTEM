use super::*;

async fn named(h: &Http, telegram_id: i64, name: &str) -> ClientId {
    h.services()
        .clients()
        .ensure_telegram(
            &TelegramProfile {
                id: telegram_id,
                username: Some(format!("u{telegram_id}")),
                first_name: Some(name.into()),
                last_name: None,
                language_code: Some("ru".into()),
                is_premium: false,
            },
            RegistrationSource::Bot,
            None,
        )
        .await
        .unwrap()
        .0
        .id
}

async fn delete(h: &Http, path: &str, token: &str) -> (StatusCode, Value) {
    let (status, _, body) = h.send(Method::DELETE, path, Some(token), None).await;
    (status, body)
}

async fn set(h: &Http, client: ClientId, referrer: i64, token: &str) -> (StatusCode, Value) {
    h.write(
        Method::PUT,
        &format!("/api/admin/clients/{}/referrer", client.0),
        token,
        json!({ "referrer_id": referrer }),
    )
    .await
}

#[tokio::test]
async fn рефералы_в_карточке_по_правам_клиентов_и_денег() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let support = h.admin("support@example.com", AdminRole::Support).await;
    let viewer = h.admin("viewer@example.com", AdminRole::ReadOnly).await;
    let alice = named(&h, 8_100_001, "Алиса").await;
    let vera = named(&h, 8_100_002, "Вера").await;

    let section = |id: ClientId| format!("/api/admin/clients/{}/referrals", id.0);
    let (status, body) = h.get(&section(vera), &viewer).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["inviter"].is_null());
    assert_eq!(body["invited_count"], 0);
    assert!(body.get("money").is_none(), "{body}");

    let (status, _) = set(&h, vera, alice.0, &viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for path in [
        format!("/api/admin/clients/{}/referrer", vera.0),
        format!(
            "/api/admin/clients/{}/referrals/invited/{}",
            alice.0, vera.0
        ),
    ] {
        let (status, _) = delete(&h, &path, &viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = h
        .get(
            &format!("/api/admin/clients/{}/referrer/candidates?q=u81", vera.0),
            &viewer,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = set(&h, vera, alice.0, &support).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], true);
    let (status, body) = h.get(&section(vera), &support).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["inviter"]["client"]["client_id"], alice.0);
    assert_eq!(body["inviter"]["client"]["name"], "Алиса");
    assert_eq!(body["inviter"]["manual"]["by"], "support@example.com");
    assert!(body.get("money").is_none(), "{body}");

    let (status, body) = h.get(&section(alice), &support).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["invited_count"], 1);
    let row = &body["invited"]["items"][0];
    assert_eq!(row["client"]["client_id"], vera.0);
    assert!(row["manual_at"].is_string(), "{row}");
    assert!(
        row.get("paid").is_none() && row.get("bonus").is_none(),
        "{row}"
    );

    let (status, body) = h.get(&section(alice), &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["money"]["invited_paid"], 0);
    assert_eq!(body["money"]["earned"]["minor"], 0);
    assert_eq!(body["invited"]["items"][0]["paid"], false);
    assert_eq!(body["invited"]["items"][0]["bonus"]["minor"], 0);
    let (status, body) = h
        .get(
            &format!("/api/admin/clients/{}/referrals/invited?limit=1", alice.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert!(body["next"].is_null());

    let (status, found) = h
        .get(
            &format!("/api/admin/clients/{}/referrer/candidates?q=u8100", vera.0),
            &support,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let refusal = |id: ClientId| {
        found
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["client"]["client_id"] == id.0)
            .map(|c| c["refusal"].clone())
    };
    assert_eq!(refusal(vera), Some(json!("itself")));
    assert_eq!(refusal(alice), Some(json!("current")));

    let (status, body) = delete(
        &h,
        &format!(
            "/api/admin/clients/{}/referrals/invited/{}",
            alice.0, vera.0
        ),
        &support,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], true);
    let (_, body) = h.get(&section(vera), &owner).await;
    assert!(body["inviter"].is_null());
}

#[tokio::test]
async fn защиты_рефералов_и_запись_в_журнал_действий() {
    let Some(h) = http().await else { return };
    let owner = h.owner().await;
    let alice = named(&h, 8_200_001, "Алиса").await;
    let boris = named(&h, 8_200_002, "Борис").await;
    let vera = named(&h, 8_200_003, "Вера").await;

    let (status, body) = set(&h, vera, vera.0, &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "referral.self");
    let (status, _) = set(&h, boris, alice.0, &owner).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = set(&h, vera, boris.0, &owner).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = set(&h, alice, vera.0, &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "referral.cycle");
    let (status, body) = set(&h, alice, 987_654_321, &owner).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "referral.unknown");
    let (status, body) = delete(
        &h,
        &format!(
            "/api/admin/clients/{}/referrals/invited/{}",
            alice.0, vera.0
        ),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "referral.not_invited");
    let (status, _) = set(&h, vera, alice.0, &owner).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = delete(
        &h,
        &format!("/api/admin/clients/{}/referrer", vera.0),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, page) = h
        .get(
            &format!(
                "/api/admin/audit?client={}&action=client.referrer_set",
                vera.0
            ),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    let changed = &items[0];
    assert_eq!(changed["title"], "Сменён пригласивший");
    assert_eq!(changed["actor"]["email"], "owner@example.com", "{changed}");
    let change = &changed["changes"][0];
    assert_eq!(change["label"], "Пригласивший");
    assert_eq!(
        change["before"],
        format!("Борис · @u8200002 · №{}", boris.0)
    );
    assert_eq!(change["after"], format!("Алиса · @u8200001 · №{}", alice.0));
    assert_eq!(items[1]["title"], "Назначен пригласивший");
    assert_eq!(items[1]["changes"][0]["before"], "—");

    let (status, page) = h
        .get(
            &format!(
                "/api/admin/audit?client={}&action=client.referrer_removed",
                vera.0
            ),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let removed = &page["items"][0];
    assert_eq!(removed["title"], "Убран пригласивший");
    assert_eq!(removed["changes"][0]["after"], "—");

    let (status, history) = h
        .get(
            &format!("/api/admin/clients/{}/history?limit=20", alice.0),
            &owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let actions: Vec<&str> = history["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["action"].as_str())
        .collect();
    assert_eq!(
        actions
            .iter()
            .filter(|a| a.starts_with("client.referrer_"))
            .count(),
        3,
        "{history}"
    );
}
